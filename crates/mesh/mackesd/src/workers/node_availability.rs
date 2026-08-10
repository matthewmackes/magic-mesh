//! WL-UX-013 — daemon-side admission for node availability intent.
//!
//! The ledger accepts the shared
//! [`mackes_mesh_types::health::NodeAvailabilityIntent`] only after that
//! contract validates the record's currentness and lifecycle transition. Its
//! concrete output sink borrows caller-supplied durable and Bus resources; it
//! never resolves defaults. This module does not infer a node state from an
//! absent record and performs no sleep, reboot, or network action.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use mackes_mesh_types::health::{
    node_health_topic, ExpectedReturn, NodeAvailabilityAssessment, NodeAvailabilityIntent,
    NodeAvailabilityPolicy, NodeAvailabilityState, NodeAvailabilityValidationError,
    NodeConnectionType, NodeConnectivitySummary, NodeDeviceClass, MAX_NODE_AVAILABILITY_ID_BYTES,
    MAX_NODE_AVAILABILITY_INTENT_TTL_MS, NODE_AVAILABILITY_INTENT_SCHEMA_VERSION,
    NODE_HEALTH_TOPIC_PREFIX,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::{Persist, PersistError};

/// Default number of distinct node identities retained by a ledger.
pub const DEFAULT_LEDGER_NODE_CAPACITY: usize = 64;

/// Maximum number of records accepted by one deterministic fold operation.
///
/// A fold may contain more than one transition for a node, so this bound is
/// intentionally separate from the number of current node entries retained
/// by [`AvailabilityLedger`].
pub const DEFAULT_FOLD_EVENT_CAPACITY: usize = 256;

/// Maximum number of previously admitted event identities retained per node.
///
/// Generation ordering still rejects older records after they leave this
/// bounded replay window; the window additionally catches event-id reuse with
/// a newer generation.
const SEEN_EVENT_CAPACITY: usize = 64;

/// Maximum compact JSON bytes retained and published for one lifecycle intent.
///
/// The shared field bounds keep a valid intent well below this ceiling. This
/// independent output bound prevents a future wire extension from silently
/// turning one daemon lifecycle event into an unbounded local or Bus write.
pub const MAX_LIFECYCLE_INTENT_RECORD_BYTES: usize = 4 * 1024;

/// Maximum canonical per-node lifecycle topic length.
pub const MAX_LIFECYCLE_INTENT_TOPIC_BYTES: usize =
    NODE_HEALTH_TOPIC_PREFIX.len() + MAX_NODE_AVAILABILITY_ID_BYTES;

/// Maximum path depth accepted for the caller-supplied durable state file.
const MAX_LIFECYCLE_INTENT_PATH_COMPONENTS: usize = 64;

/// Fresh non-absence records remain usable long enough for ordinary daemon
/// reconciliation without masquerading as an indefinite heartbeat.
const RUNTIME_RETURN_TTL: Duration = Duration::from_secs(10 * 60);

/// Additional admission lifetime after a declared return deadline. The health
/// policy owns warning/critical timing; this merely keeps the intent available
/// while that policy evaluates a missed return.
const RUNTIME_EXPECTED_RETURN_GRACE: Duration = Duration::from_secs(10 * 60);

/// Serialize the one node-owned generation across independently scheduled
/// daemon workers. The durable record remains the restart authority.
static RUNTIME_PUBLICATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// A fixed-capacity, latest-intent ledger keyed by node identity.
///
/// The const parameter bounds the number of distinct nodes. Existing nodes
/// may advance their current intent without consuming another slot. Entries
/// are held in a [`BTreeMap`] so snapshots and iteration are deterministic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailabilityLedger<const MAX_NODES: usize = DEFAULT_LEDGER_NODE_CAPACITY> {
    entries: BTreeMap<String, LedgerEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LedgerEntry {
    current: NodeAvailabilityIntent,
    seen_event_ids: VecDeque<String>,
}

impl<const MAX_NODES: usize> Default for AvailabilityLedger<MAX_NODES> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const MAX_NODES: usize> AvailabilityLedger<MAX_NODES> {
    /// Construct an empty ledger.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Return the compile-time maximum number of distinct nodes.
    #[must_use]
    pub const fn capacity() -> usize {
        MAX_NODES
    }

    /// Return the number of node entries currently retained.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the ledger has no admitted node intents.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Look up the latest accepted intent for `node_id`.
    ///
    /// `None` means that this ledger has no admitted record for the node. It
    /// is not an inferred availability state; an admitted `Unknown` intent is
    /// returned as `Some` with [`NodeAvailabilityState::Unknown`].
    #[must_use]
    pub fn current(&self, node_id: &str) -> Option<&NodeAvailabilityIntent> {
        self.entries.get(node_id).map(|entry| &entry.current)
    }

    /// Evaluate one node against explicit roster/heartbeat evidence.
    ///
    /// The ledger supplies only an admitted current intent. The caller must
    /// supply the independently observed device class and last-seen timestamp;
    /// missing or contradictory evidence remains
    /// [`NodeAvailabilityAssessment::Unknown`]. All actual classification and
    /// device-aware thresholds are delegated to the shared
    /// [`NodeAvailabilityPolicy`].
    #[must_use]
    pub fn assess_node(
        &self,
        node_id: &str,
        now_ms: u64,
        evidence: Option<&NodeAvailabilityEvidence>,
    ) -> NodeAvailabilityAssessment {
        let Some(evidence) = evidence.filter(|evidence| evidence.node_id == node_id) else {
            return NodeAvailabilityAssessment::Unknown;
        };
        let intent = self.current(node_id);
        if intent.is_some_and(|intent| intent.device_class != evidence.device_class) {
            return NodeAvailabilityAssessment::Unknown;
        }

        NodeAvailabilityPolicy::for_device_class(evidence.device_class).assess(
            intent,
            now_ms,
            evidence.last_seen_at_ms,
        )
    }

    /// Evaluate the admitted ledger and explicit evidence as a bounded,
    /// deterministic snapshot.
    ///
    /// The result contains the union of admitted nodes and evidenced nodes in
    /// lexicographic node-id order. That union may not exceed this ledger's
    /// node capacity. Duplicate evidence is rejected instead of making input
    /// delivery order authoritative.
    pub fn evaluate(
        &self,
        evidence: impl IntoIterator<Item = NodeAvailabilityEvidence>,
        now_ms: u64,
    ) -> Result<AvailabilityEvaluationSnapshot, AvailabilityEvaluationError> {
        let mut evidence_by_node = BTreeMap::new();
        for item in evidence {
            let node_id = item.node_id.clone();
            if evidence_by_node.contains_key(&node_id) {
                return Err(AvailabilityEvaluationError::DuplicateEvidence { node_id });
            }
            if evidence_by_node.len() >= MAX_NODES {
                return Err(AvailabilityEvaluationError::CapacityExceeded {
                    capacity: MAX_NODES,
                });
            }
            evidence_by_node.insert(node_id, item);
        }

        let mut assessments = BTreeMap::new();
        for node_id in self.entries.keys() {
            assessments.insert(
                node_id.clone(),
                self.assess_node(node_id, now_ms, evidence_by_node.get(node_id)),
            );
        }
        for (node_id, item) in &evidence_by_node {
            assessments
                .entry(node_id.clone())
                .or_insert_with(|| self.assess_node(node_id, now_ms, Some(item)));
        }
        if assessments.len() > MAX_NODES {
            return Err(AvailabilityEvaluationError::CapacityExceeded {
                capacity: MAX_NODES,
            });
        }

        Ok(AvailabilityEvaluationSnapshot {
            assessments: assessments
                .into_iter()
                .map(|(node_id, assessment)| NodeAvailabilityEvaluation {
                    node_id,
                    assessment,
                })
                .collect(),
        })
    }

    /// Iterate over current intents in lexicographic node-id order.
    pub fn iter(&self) -> impl Iterator<Item = &NodeAvailabilityIntent> {
        self.entries.values().map(|entry| &entry.current)
    }

    /// Admit one current intent at `now_ms`.
    ///
    /// The shared health contract performs shape, expiry, generation, replay,
    /// and lifecycle-transition validation. This ledger adds the fixed node
    /// capacity and a bounded per-node event-id replay window. Failed
    /// admissions leave the ledger unchanged.
    pub fn admit(
        &mut self,
        intent: NodeAvailabilityIntent,
        now_ms: u64,
    ) -> Result<AdmissionReceipt, AvailabilityAdmissionError> {
        let node_id = intent.node_id.clone();
        let previous = self.entries.get(&node_id);

        intent
            .validate_transition(previous.map(|entry| &entry.current), now_ms)
            .map_err(AvailabilityAdmissionError::Contract)?;

        if let Some(previous) = previous {
            if previous
                .seen_event_ids
                .iter()
                .any(|event_id| event_id == &intent.event_id)
            {
                return Err(AvailabilityAdmissionError::Contract(
                    NodeAvailabilityValidationError::Replay,
                ));
            }
        } else if self.entries.len() >= MAX_NODES {
            return Err(AvailabilityAdmissionError::CapacityExceeded {
                capacity: MAX_NODES,
                node_id,
            });
        }

        let generation = intent.generation;
        let state = intent.state;
        let replaced = self.entries.contains_key(&intent.node_id);
        let mut seen_event_ids = previous
            .map(|entry| entry.seen_event_ids.clone())
            .unwrap_or_default();
        if seen_event_ids.len() == SEEN_EVENT_CAPACITY {
            seen_event_ids.pop_front();
        }
        seen_event_ids.push_back(intent.event_id.clone());
        self.entries.insert(
            intent.node_id.clone(),
            LedgerEntry {
                current: intent,
                seen_event_ids,
            },
        );

        Ok(AdmissionReceipt {
            node_id,
            generation,
            state,
            replaced,
        })
    }

    /// Validate, durably retain, and publish explicit lifecycle evidence.
    ///
    /// The caller must identify the lifecycle event and supply its complete
    /// typed intent. The evidence kind is checked against the intent state,
    /// then the shared transition policy and this ledger's replay/capacity
    /// bounds are applied before the injected output seam is called. Missing
    /// heartbeat evidence, elapsed wall-clock time, or node absence cannot
    /// enter through this API and are never converted into lifecycle intent.
    ///
    /// Admission is staged on a bounded clone. The local ledger changes only
    /// after the sink confirms that it persisted and published the exact
    /// admitted record. The sink owns atomicity of those two output lanes and
    /// must return an error unless both completed.
    pub fn publish_lifecycle_intent<S: LifecycleIntentSink>(
        &mut self,
        evidence: LifecycleIntentEvidence,
        now_ms: u64,
        sink: &mut S,
    ) -> Result<AdmissionReceipt, LifecycleIntentPublicationError<S::Error>> {
        evidence
            .validate_kind()
            .map_err(AvailabilityAdmissionError::Contract)
            .map_err(LifecycleIntentPublicationError::Admission)?;
        let intent = evidence.into_intent();
        let mut staged = self.clone();
        let receipt = staged
            .admit(intent.clone(), now_ms)
            .map_err(LifecycleIntentPublicationError::Admission)?;

        sink.persist_and_publish(&intent)
            .map_err(LifecycleIntentPublicationError::Output)?;
        *self = staged;
        Ok(receipt)
    }

    /// Produce a deterministic snapshot of the current accepted intents.
    #[must_use]
    pub fn snapshot(&self) -> AvailabilitySnapshot {
        AvailabilitySnapshot {
            intents: self.iter().cloned().collect(),
        }
    }

    /// Fold a bounded set of intents in canonical node/generation order.
    ///
    /// Sorting makes a replay of the same event stream independent of input
    /// delivery order. Every record must still be current at `now_ms`; this
    /// helper is for an admission fold, not for silently reviving expired
    /// history.
    pub fn fold(
        intents: impl IntoIterator<Item = NodeAvailabilityIntent>,
        now_ms: u64,
    ) -> Result<Self, AvailabilityFoldError> {
        let mut ordered = Vec::new();
        for intent in intents {
            if ordered.len() == DEFAULT_FOLD_EVENT_CAPACITY {
                return Err(AvailabilityFoldError::TooManyEvents {
                    capacity: DEFAULT_FOLD_EVENT_CAPACITY,
                });
            }
            ordered.push(intent);
        }

        ordered.sort_by(|left, right| {
            left.node_id
                .cmp(&right.node_id)
                .then(left.generation.cmp(&right.generation))
                .then(left.observed_at_ms.cmp(&right.observed_at_ms))
                .then(left.event_id.cmp(&right.event_id))
                .then_with(|| format!("{left:?}").cmp(&format!("{right:?}")))
        });

        let mut ledger = Self::new();
        for intent in ordered {
            ledger
                .admit(intent, now_ms)
                .map_err(AvailabilityFoldError::Admission)?;
        }
        Ok(ledger)
    }

    /// Re-admit a previously captured snapshot through the same currentness
    /// and capacity policy.
    pub fn fold_snapshot(
        snapshot: &AvailabilitySnapshot,
        now_ms: u64,
    ) -> Result<Self, AvailabilityFoldError> {
        Self::fold(snapshot.intents.clone(), now_ms)
    }
}

/// Explicit daemon lifecycle evidence accepted by the publication seam.
///
/// Each variant must carry a matching shared intent. This extra closed tag
/// prevents a caller from labeling an `Awake` or `Unknown` observation as a
/// planned absence. Expected-absence variants also remain subject to the
/// shared contract's mandatory, bounded [`ExpectedReturn`](mackes_mesh_types::health::ExpectedReturn).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleIntentEvidence {
    /// A logind sleep lifecycle event.
    Sleep(NodeAvailabilityIntent),
    /// A managed shutdown lifecycle event, including its shut-down phase.
    PlannedShutdown(NodeAvailabilityIntent),
    /// A staged reboot lifecycle event, including its rebooting phase.
    PlannedReboot(NodeAvailabilityIntent),
    /// Entry into an explicit maintenance window.
    Maintenance(NodeAvailabilityIntent),
    /// A NetworkManager adapter/address transition.
    AdapterTransition(NodeAvailabilityIntent),
    /// Explicit resume, boot, or connectivity-stabilized return evidence.
    Returned(NodeAvailabilityIntent),
}

impl LifecycleIntentEvidence {
    fn intent(&self) -> &NodeAvailabilityIntent {
        match self {
            Self::Sleep(intent)
            | Self::PlannedShutdown(intent)
            | Self::PlannedReboot(intent)
            | Self::Maintenance(intent)
            | Self::AdapterTransition(intent)
            | Self::Returned(intent) => intent,
        }
    }

    fn into_intent(self) -> NodeAvailabilityIntent {
        match self {
            Self::Sleep(intent)
            | Self::PlannedShutdown(intent)
            | Self::PlannedReboot(intent)
            | Self::Maintenance(intent)
            | Self::AdapterTransition(intent)
            | Self::Returned(intent) => intent,
        }
    }

    fn validate_kind(&self) -> Result<(), NodeAvailabilityValidationError> {
        let state = self.intent().state;
        let matches_kind = match self {
            Self::Sleep(_) => state == NodeAvailabilityState::Sleeping,
            Self::PlannedShutdown(_) => matches!(
                state,
                NodeAvailabilityState::ShuttingDown | NodeAvailabilityState::ShutDown
            ),
            Self::PlannedReboot(_) => matches!(
                state,
                NodeAvailabilityState::ScheduledReboot | NodeAvailabilityState::Rebooting
            ),
            Self::Maintenance(_) => state == NodeAvailabilityState::Maintenance,
            Self::AdapterTransition(_) => state == NodeAvailabilityState::AdapterMigration,
            Self::Returned(_) => state == NodeAvailabilityState::Returned,
        };
        if matches_kind {
            Ok(())
        } else {
            Err(NodeAvailabilityValidationError::Contradictory(
                "lifecycle evidence kind does not match intent state",
            ))
        }
    }
}

/// Injected durable-publication boundary for one admitted lifecycle intent.
///
/// Implementations must retain and publish the supplied record exactly and
/// return `Ok(())` only when both lanes succeeded. This policy module does not
/// select a default store, Bus handle, node identity, or clock.
pub trait LifecycleIntentSink {
    /// Caller-specific output failure.
    type Error;

    /// Persist and publish one already validated intent as one output action.
    fn persist_and_publish(&mut self, intent: &NodeAvailabilityIntent) -> Result<(), Self::Error>;
}

/// Production lifecycle output backed by one caller-owned Bus handle and one
/// exact caller-supplied durable state path.
///
/// Construction opens neither resource and resolves no default. The sink
/// compact-encodes an already-admitted intent exactly once, atomically replaces
/// the local record, then publishes those identical UTF-8 bytes to the shared
/// canonical per-node health topic. A Bus failure deliberately leaves the
/// durable record available for retry; [`AvailabilityLedger`] remains
/// unchanged until this sink returns success.
pub struct PersistLifecycleIntentSink<'a> {
    persist: &'a mut Persist,
    durable_path: &'a Path,
    bus_root: &'a Path,
    bus_identity: AvailabilityBusIdentity,
    #[cfg(test)]
    replacement_after_write: Option<(PathBuf, PathBuf)>,
}

impl<'a> PersistLifecycleIntentSink<'a> {
    /// Bind the caller's exact Bus connection to its current path generation
    /// and borrow the exact durable record path.
    pub(crate) fn new(
        persist: &'a mut Persist,
        durable_path: &'a Path,
        bus_root: &'a Path,
    ) -> Result<Self, PersistLifecycleIntentError> {
        let bus_identity = availability_bus_identity(bus_root)
            .map_err(PersistLifecycleIntentError::BusIdentity)?;
        if persist.index_inode() != Some(bus_identity.inode) {
            return Err(PersistLifecycleIntentError::BusIdentity(
                "Bus connection does not match the current index path".to_string(),
            ));
        }
        Ok(Self {
            persist,
            durable_path,
            bus_root,
            bus_identity,
            #[cfg(test)]
            replacement_after_write: None,
        })
    }

    /// Return the exact caller-supplied durable record path.
    #[must_use]
    pub const fn durable_path(&self) -> &Path {
        self.durable_path
    }

    #[cfg(test)]
    fn with_replacement_after_write(
        mut self,
        replacement_root: PathBuf,
        retired_root: PathBuf,
    ) -> Self {
        self.replacement_after_write = Some((replacement_root, retired_root));
        self
    }

    fn verify_bus(&self) -> Result<(), PersistLifecycleIntentError> {
        verify_availability_bus_identity(self.persist, self.bus_root, self.bus_identity)
            .map_err(PersistLifecycleIntentError::BusIdentity)
    }
}

impl LifecycleIntentSink for PersistLifecycleIntentSink<'_> {
    type Error = PersistLifecycleIntentError;

    fn persist_and_publish(&mut self, intent: &NodeAvailabilityIntent) -> Result<(), Self::Error> {
        let body = serde_json::to_string(intent)
            .map_err(|error| PersistLifecycleIntentError::Encoding(error.to_string()))?;
        if body.len() > MAX_LIFECYCLE_INTENT_RECORD_BYTES {
            return Err(PersistLifecycleIntentError::RecordTooLarge {
                bytes: body.len(),
                max: MAX_LIFECYCLE_INTENT_RECORD_BYTES,
            });
        }

        let topic = node_health_topic(&intent.node_id);
        if topic.len() > MAX_LIFECYCLE_INTENT_TOPIC_BYTES {
            return Err(PersistLifecycleIntentError::TopicTooLong {
                bytes: topic.len(),
                max: MAX_LIFECYCLE_INTENT_TOPIC_BYTES,
            });
        }

        self.verify_bus()?;
        write_lifecycle_intent_record(self.durable_path, body.as_bytes())?;
        self.verify_bus()?;
        self.persist
            .write(&topic, Priority::Default, None, Some(&body))
            .map_err(PersistLifecycleIntentError::Bus)?;

        #[cfg(test)]
        if let Some((replacement_root, retired_root)) = self.replacement_after_write.take() {
            std::fs::rename(self.bus_root, retired_root)
                .map_err(|error| PersistLifecycleIntentError::BusIdentity(error.to_string()))?;
            std::fs::rename(replacement_root, self.bus_root)
                .map_err(|error| PersistLifecycleIntentError::BusIdentity(error.to_string()))?;
        }

        self.verify_bus()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AvailabilityBusIdentity {
    device: u64,
    inode: u64,
}

fn availability_bus_identity(root: &Path) -> Result<AvailabilityBusIdentity, String> {
    let index = root.join("index.sqlite");
    let metadata = std::fs::metadata(&index)
        .map_err(|error| format!("inspect Bus index {}: {error}", index.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "Bus index {} is not a regular file",
            index.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        Ok(AvailabilityBusIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Ok(AvailabilityBusIdentity {
            device: 0,
            inode: 0,
        })
    }
}

fn open_current_availability_bus(
    root: &Path,
) -> Result<(Persist, AvailabilityBusIdentity), RuntimeAvailabilityError> {
    let before = match availability_bus_identity(root) {
        Ok(identity) => identity,
        Err(_) if !root.join("index.sqlite").exists() => {
            drop(Persist::open(root.to_path_buf()).map_err(RuntimeAvailabilityError::Bus)?);
            availability_bus_identity(root).map_err(RuntimeAvailabilityError::BusIdentity)?
        }
        Err(error) => return Err(RuntimeAvailabilityError::BusIdentity(error)),
    };
    let persist = Persist::open(root.to_path_buf()).map_err(RuntimeAvailabilityError::Bus)?;
    let after = availability_bus_identity(root).map_err(RuntimeAvailabilityError::BusIdentity)?;
    if before != after || persist.index_inode() != Some(after.inode) {
        return Err(RuntimeAvailabilityError::BusIdentity(
            "Bus connection/path identity changed while opening".to_string(),
        ));
    }
    Ok((persist, after))
}

fn verify_availability_bus_identity(
    persist: &Persist,
    root: &Path,
    expected: AvailabilityBusIdentity,
) -> Result<(), String> {
    let current = availability_bus_identity(root)?;
    if current != expected || persist.index_inode() != Some(expected.inode) {
        return Err("Bus connection/path identity changed during transaction".to_string());
    }
    Ok(())
}

/// One runtime producer request. Callers choose a closed lifecycle state and
/// exact bounded source/reason; this layer owns identity, event generation,
/// expiry construction, corrected-forward retry, and sink admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeAvailabilityRequest {
    /// Closed lifecycle state being reported.
    pub state: NodeAvailabilityState,
    /// Stable producer identity, subject to the shared identifier bound.
    pub source: &'static str,
    /// Operator-readable reason, subject to the shared reason bound.
    pub reason: String,
    /// Expected-return delay for expected-absence states only.
    pub expected_return_after: Option<Duration>,
    /// Connectivity before an adapter migration, or the prior connectivity on
    /// its corresponding returned record.
    pub old_connectivity: Option<NodeConnectivitySummary>,
    /// Intended/stabilized connectivity after a migration.
    pub new_connectivity: Option<NodeConnectivitySummary>,
}

impl RuntimeAvailabilityRequest {
    /// Construct a lifecycle request without connectivity evidence.
    #[must_use]
    pub fn lifecycle(
        state: NodeAvailabilityState,
        source: &'static str,
        reason: impl Into<String>,
        expected_return_after: Option<Duration>,
    ) -> Self {
        Self {
            state,
            source,
            reason: reason.into(),
            expected_return_after,
            old_connectivity: None,
            new_connectivity: None,
        }
    }

    /// Attach exact old/new connectivity evidence.
    #[must_use]
    pub fn with_connectivity(
        mut self,
        old_connectivity: NodeConnectivitySummary,
        new_connectivity: NodeConnectivitySummary,
    ) -> Self {
        self.old_connectivity = Some(old_connectivity);
        self.new_connectivity = Some(new_connectivity);
        self
    }
}

/// Runtime owner for the landed persistence/publication sink.
///
/// Each call reopens the shared Bus, serializes against other in-process
/// lifecycle producers, and treats the durable record as the restart authority.
/// If a prior sink attempt persisted but failed to publish, its exact bytes are
/// published before any newer generation is admitted. An idempotent retry of
/// that same transition then returns the existing record instead of minting a
/// duplicate event.
#[derive(Debug, Clone)]
pub struct RuntimeAvailabilityPublisher {
    node_id: String,
    device_id: String,
    device_class: NodeDeviceClass,
    bus_root: PathBuf,
    durable_path: PathBuf,
}

impl RuntimeAvailabilityPublisher {
    /// Bind one producer to explicit node/device identity and exact resources.
    #[must_use]
    pub fn new(
        node_id: String,
        device_id: String,
        device_class: NodeDeviceClass,
        bus_root: PathBuf,
        durable_path: PathBuf,
    ) -> Self {
        Self {
            node_id,
            device_id,
            device_class,
            bus_root,
            durable_path,
        }
    }

    /// Read the latest durable intent through the same no-symlink boundary used
    /// for publication. Invalid, oversized, or wrong-identity records fail
    /// closed instead of being repaired by inference.
    pub fn current_intent(
        &self,
    ) -> Result<Option<NodeAvailabilityIntent>, RuntimeAvailabilityError> {
        let Some(body) = read_lifecycle_intent_record(&self.durable_path)? else {
            return Ok(None);
        };
        let intent: NodeAvailabilityIntent = serde_json::from_slice(&body)
            .map_err(|error| RuntimeAvailabilityError::DurableRecord(error.to_string()))?;
        intent
            .validate()
            .map_err(RuntimeAvailabilityError::Contract)?;
        if intent.node_id != self.node_id || intent.device_id != self.device_id {
            return Err(RuntimeAvailabilityError::DurableRecord(
                "durable lifecycle identity does not match this node".to_string(),
            ));
        }
        Ok(Some(intent))
    }

    /// Correct a durable-only sink result forward without minting another
    /// generation. This is useful on a no-op reconciliation pass where there is
    /// no newer transition to trigger [`Self::publish_at`].
    pub fn correct_forward(
        &self,
    ) -> Result<Option<NodeAvailabilityIntent>, RuntimeAvailabilityError> {
        let publication_lock = RUNTIME_PUBLICATION_LOCK.get_or_init(|| Mutex::new(()));
        let _guard = publication_lock
            .lock()
            .map_err(|_| RuntimeAvailabilityError::PublicationLockPoisoned)?;
        let (mut persist, bus_identity) = open_current_availability_bus(&self.bus_root)?;
        let current = self.current_intent()?;
        verify_availability_bus_identity(&persist, &self.bus_root, bus_identity)
            .map_err(RuntimeAvailabilityError::BusIdentity)?;
        if let Some(intent) = &current {
            retry_durable_publication(&mut persist, &self.bus_root, bus_identity, intent)?;
        }
        Ok(current)
    }

    /// Publish one transition at an injected wall clock.
    pub fn publish_at(
        &self,
        request: RuntimeAvailabilityRequest,
        now_ms: u64,
    ) -> Result<NodeAvailabilityIntent, RuntimeAvailabilityError> {
        let publication_lock = RUNTIME_PUBLICATION_LOCK.get_or_init(|| Mutex::new(()));
        let _guard = publication_lock
            .lock()
            .map_err(|_| RuntimeAvailabilityError::PublicationLockPoisoned)?;
        let (mut persist, bus_identity) = open_current_availability_bus(&self.bus_root)?;
        let previous = self.current_intent()?;
        verify_availability_bus_identity(&persist, &self.bus_root, bus_identity)
            .map_err(RuntimeAvailabilityError::BusIdentity)?;

        if let Some(previous) = &previous {
            retry_durable_publication(&mut persist, &self.bus_root, bus_identity, previous)?;
            if runtime_request_matches(previous, &request) && now_ms <= previous.expires_at_ms {
                return Ok(previous.clone());
            }
        }

        let intent = build_runtime_intent(
            &self.node_id,
            &self.device_id,
            self.device_class,
            previous.as_ref(),
            request,
            now_ms,
        )?;
        let mut ledger = AvailabilityLedger::<1>::new();
        if let Some(previous) = previous {
            ledger
                .admit(previous.clone(), previous.observed_at_ms)
                .map_err(RuntimeAvailabilityError::Admission)?;
        }
        let evidence = runtime_evidence(intent.clone())?;
        let mut sink =
            PersistLifecycleIntentSink::new(&mut persist, &self.durable_path, &self.bus_root)
                .map_err(RuntimeAvailabilityError::DurableFilesystem)?;
        ledger
            .publish_lifecycle_intent(evidence, now_ms, &mut sink)
            .map_err(RuntimeAvailabilityError::Publication)?;
        Ok(intent)
    }

    /// Publish with the current Unix epoch clock.
    pub fn publish(
        &self,
        request: RuntimeAvailabilityRequest,
    ) -> Result<NodeAvailabilityIntent, RuntimeAvailabilityError> {
        self.publish_at(request, availability_now_ms())
    }
}

/// Canonical durable path shared by this node's independently scheduled
/// lifecycle/network producers.
#[must_use]
pub fn runtime_availability_path(workgroup_root: &Path, node_id: &str) -> PathBuf {
    workgroup_root
        .join("fleet")
        .join("availability")
        .join(node_id)
        .join("current.json")
}

fn runtime_request_matches(
    current: &NodeAvailabilityIntent,
    request: &RuntimeAvailabilityRequest,
) -> bool {
    current.state == request.state
        && current.source == request.source
        && current.reason == request.reason
        && current.old_connectivity == request.old_connectivity
        && current.new_connectivity == request.new_connectivity
}

fn build_runtime_intent(
    node_id: &str,
    device_id: &str,
    device_class: NodeDeviceClass,
    previous: Option<&NodeAvailabilityIntent>,
    request: RuntimeAvailabilityRequest,
    now_ms: u64,
) -> Result<NodeAvailabilityIntent, RuntimeAvailabilityError> {
    let generation = previous.map_or(1, |intent| intent.generation.saturating_add(1));
    if generation == 0 || previous.is_some_and(|intent| generation <= intent.generation) {
        return Err(RuntimeAvailabilityError::GenerationExhausted);
    }
    let expected_return = match request.expected_return_after {
        Some(delay) => {
            if !request.state.expects_return() {
                return Err(RuntimeAvailabilityError::Contract(
                    NodeAvailabilityValidationError::ExpectedReturnForbidden,
                ));
            }
            let delay_ms = duration_ms(delay)?;
            Some(ExpectedReturn::new(now_ms.saturating_add(delay_ms)))
        }
        None if request.state.expects_return() => {
            return Err(RuntimeAvailabilityError::Contract(
                NodeAvailabilityValidationError::ExpectedReturnRequired,
            ));
        }
        None => None,
    };
    let expiry_delay = match &expected_return {
        Some(expected) => expected
            .expected_at_ms
            .saturating_sub(now_ms)
            .saturating_add(duration_ms(RUNTIME_EXPECTED_RETURN_GRACE)?),
        None => duration_ms(RUNTIME_RETURN_TTL)?,
    };
    if expiry_delay > MAX_NODE_AVAILABILITY_INTENT_TTL_MS {
        return Err(RuntimeAvailabilityError::Contract(
            NodeAvailabilityValidationError::ExpiryTooFar,
        ));
    }
    let connection_type = request
        .old_connectivity
        .as_ref()
        .or(request.new_connectivity.as_ref())
        .map_or(NodeConnectionType::Unknown, |summary| {
            summary.connection_type
        });
    let intent = NodeAvailabilityIntent {
        schema_version: NODE_AVAILABILITY_INTENT_SCHEMA_VERSION,
        node_id: node_id.to_string(),
        device_id: device_id.to_string(),
        device_class,
        connection_type,
        state: request.state,
        reason: request.reason,
        source: request.source.to_string(),
        event_id: format!("availability-{generation}-{now_ms}"),
        generation,
        observed_at_ms: now_ms,
        expires_at_ms: now_ms.saturating_add(expiry_delay),
        expected_return,
        old_connectivity: request.old_connectivity,
        new_connectivity: request.new_connectivity,
    };
    intent
        .validate_transition(previous, now_ms)
        .map_err(RuntimeAvailabilityError::Contract)?;
    Ok(intent)
}

fn duration_ms(duration: Duration) -> Result<u64, RuntimeAvailabilityError> {
    u64::try_from(duration.as_millis()).map_err(|_| {
        RuntimeAvailabilityError::Contract(NodeAvailabilityValidationError::ExpiryTooFar)
    })
}

fn runtime_evidence(
    intent: NodeAvailabilityIntent,
) -> Result<LifecycleIntentEvidence, RuntimeAvailabilityError> {
    match intent.state {
        NodeAvailabilityState::Sleeping => Ok(LifecycleIntentEvidence::Sleep(intent)),
        NodeAvailabilityState::ShuttingDown | NodeAvailabilityState::ShutDown => {
            Ok(LifecycleIntentEvidence::PlannedShutdown(intent))
        }
        NodeAvailabilityState::ScheduledReboot | NodeAvailabilityState::Rebooting => {
            Ok(LifecycleIntentEvidence::PlannedReboot(intent))
        }
        NodeAvailabilityState::Maintenance => Ok(LifecycleIntentEvidence::Maintenance(intent)),
        NodeAvailabilityState::AdapterMigration => {
            Ok(LifecycleIntentEvidence::AdapterTransition(intent))
        }
        NodeAvailabilityState::Returned => Ok(LifecycleIntentEvidence::Returned(intent)),
        state => Err(RuntimeAvailabilityError::UnsupportedRuntimeState(state)),
    }
}

fn retry_durable_publication(
    persist: &mut Persist,
    bus_root: &Path,
    bus_identity: AvailabilityBusIdentity,
    intent: &NodeAvailabilityIntent,
) -> Result<(), RuntimeAvailabilityError> {
    let body = serde_json::to_string(intent)
        .map_err(|error| RuntimeAvailabilityError::DurableRecord(error.to_string()))?;
    if body.len() > MAX_LIFECYCLE_INTENT_RECORD_BYTES {
        return Err(RuntimeAvailabilityError::DurableRecord(
            "durable lifecycle record exceeds the publication byte bound".to_string(),
        ));
    }
    let topic = node_health_topic(&intent.node_id);
    if topic.len() > MAX_LIFECYCLE_INTENT_TOPIC_BYTES {
        return Err(RuntimeAvailabilityError::DurableRecord(
            "durable lifecycle topic exceeds the publication byte bound".to_string(),
        ));
    }
    verify_availability_bus_identity(persist, bus_root, bus_identity)
        .map_err(RuntimeAvailabilityError::BusIdentity)?;
    let latest = persist
        .read_latest(&topic)
        .map_err(RuntimeAvailabilityError::Bus)?;
    verify_availability_bus_identity(persist, bus_root, bus_identity)
        .map_err(RuntimeAvailabilityError::BusIdentity)?;
    let already_published = match latest.and_then(|message| message.body) {
        None => false,
        Some(published) if published == body => true,
        Some(published) => {
            if published.len() > MAX_LIFECYCLE_INTENT_RECORD_BYTES {
                return Err(RuntimeAvailabilityError::BusProjection(
                    "latest Bus availability row exceeds the byte bound".to_string(),
                ));
            }
            let retained: NodeAvailabilityIntent =
                serde_json::from_str(&published).map_err(|error| {
                    RuntimeAvailabilityError::BusProjection(format!(
                        "latest Bus availability row is malformed: {error}"
                    ))
                })?;
            retained
                .validate()
                .map_err(|error| RuntimeAvailabilityError::BusProjection(error.to_string()))?;
            if retained.node_id != intent.node_id || retained.device_id != intent.device_id {
                return Err(RuntimeAvailabilityError::BusProjection(
                    "latest Bus availability row carries another identity".to_string(),
                ));
            }
            if retained.generation >= intent.generation {
                return Err(RuntimeAvailabilityError::BusProjection(
                    "latest Bus availability row conflicts with retained durable truth".to_string(),
                ));
            }
            false
        }
    };
    if !already_published {
        persist
            .write(&topic, Priority::Default, None, Some(&body))
            .map_err(RuntimeAvailabilityError::Bus)?;
        verify_availability_bus_identity(persist, bus_root, bus_identity)
            .map_err(RuntimeAvailabilityError::BusIdentity)?;
    }
    Ok(())
}

fn availability_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(1)
        .max(1)
}

/// Runtime publication failure. Callers must fail closed before the lifecycle
/// mutation when this is returned.
#[derive(Debug)]
pub enum RuntimeAvailabilityError {
    /// The shared contract rejected source/reason/state/timing/connectivity.
    Contract(NodeAvailabilityValidationError),
    /// Reconstructing the previous generation failed admission.
    Admission(AvailabilityAdmissionError),
    /// The landed atomic sink rejected persistence or publication.
    Publication(LifecycleIntentPublicationError<PersistLifecycleIntentError>),
    /// Opening or inspecting the Bus failed.
    Bus(PersistError),
    /// The live Bus index path and opened SQLite connection were not one
    /// stable storage generation for the complete transaction.
    BusIdentity(String),
    /// The bounded retained availability projection was malformed or
    /// contradicted the durable publication outbox.
    BusProjection(String),
    /// The durable record was malformed or carried another identity.
    DurableRecord(String),
    /// The generation counter cannot advance safely.
    GenerationExhausted,
    /// Another publisher panicked while holding the process-wide lock.
    PublicationLockPoisoned,
    /// Awake/unknown are observations, not accepted lifecycle producer claims.
    UnsupportedRuntimeState(NodeAvailabilityState),
    /// Secure durable-record access failed.
    DurableFilesystem(PersistLifecycleIntentError),
}

impl fmt::Display for RuntimeAvailabilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(error) => write!(formatter, "availability contract: {error}"),
            Self::Admission(error) => write!(formatter, "availability admission: {error}"),
            Self::Publication(error) => write!(formatter, "availability publication: {error}"),
            Self::Bus(error) => write!(formatter, "availability Bus: {error}"),
            Self::BusIdentity(error) => write!(formatter, "availability Bus identity: {error}"),
            Self::BusProjection(error) => {
                write!(formatter, "availability Bus projection: {error}")
            }
            Self::DurableRecord(error) => write!(formatter, "availability durable record: {error}"),
            Self::GenerationExhausted => formatter.write_str("availability generation exhausted"),
            Self::PublicationLockPoisoned => {
                formatter.write_str("availability publication lock poisoned")
            }
            Self::UnsupportedRuntimeState(state) => {
                write!(
                    formatter,
                    "unsupported runtime availability state {state:?}"
                )
            }
            Self::DurableFilesystem(error) => {
                write!(formatter, "availability durable filesystem: {error}")
            }
        }
    }
}

impl std::error::Error for RuntimeAvailabilityError {}

/// Explicit production lifecycle output failure lane.
#[derive(Debug)]
pub enum PersistLifecycleIntentError {
    /// Compact typed JSON encoding failed before either output lane ran.
    Encoding(String),
    /// The encoded record exceeded the independent output byte ceiling.
    RecordTooLarge {
        /// Encoded body bytes.
        bytes: usize,
        /// Maximum admitted body bytes.
        max: usize,
    },
    /// The canonical node topic exceeded its shared identity-derived bound.
    TopicTooLong {
        /// Canonical topic bytes.
        bytes: usize,
        /// Maximum admitted topic bytes.
        max: usize,
    },
    /// The exact durable path was not absolute, had no file name, or exceeded
    /// the bounded component depth.
    InvalidDurablePath(&'static str),
    /// A symlink appeared in the exact durable path and was rejected.
    SymlinkRejected,
    /// A local durable-record operation failed.
    Filesystem {
        /// Closed operation name; no record contents are included.
        operation: &'static str,
        /// Operating-system failure detail.
        detail: String,
    },
    /// The canonical Bus write failed after durable retention.
    Bus(PersistError),
    /// The opened SQLite connection and canonical index path did not remain
    /// bound to one storage generation through publication verification.
    BusIdentity(String),
}

impl fmt::Display for PersistLifecycleIntentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encoding(error) => write!(formatter, "encode lifecycle intent: {error}"),
            Self::RecordTooLarge { bytes, max } => {
                write!(
                    formatter,
                    "lifecycle intent record is {bytes} bytes; maximum is {max}"
                )
            }
            Self::TopicTooLong { bytes, max } => {
                write!(
                    formatter,
                    "lifecycle intent topic is {bytes} bytes; maximum is {max}"
                )
            }
            Self::InvalidDurablePath(reason) => {
                write!(formatter, "invalid lifecycle intent durable path: {reason}")
            }
            Self::SymlinkRejected => {
                formatter.write_str("lifecycle intent durable path contains a symlink")
            }
            Self::Filesystem { operation, detail } => {
                write!(formatter, "lifecycle intent {operation} failed: {detail}")
            }
            Self::Bus(error) => write!(formatter, "lifecycle intent Bus publish failed: {error}"),
            Self::BusIdentity(error) => {
                write!(formatter, "lifecycle intent Bus identity failed: {error}")
            }
        }
    }
}

impl std::error::Error for PersistLifecycleIntentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Bus(error) => Some(error),
            Self::Encoding(_)
            | Self::RecordTooLarge { .. }
            | Self::TopicTooLong { .. }
            | Self::InvalidDurablePath(_)
            | Self::SymlinkRejected
            | Self::BusIdentity(_)
            | Self::Filesystem { .. } => None,
        }
    }
}

fn lifecycle_filesystem_error(
    operation: &'static str,
    error: impl fmt::Display,
) -> PersistLifecycleIntentError {
    PersistLifecycleIntentError::Filesystem {
        operation,
        detail: error.to_string(),
    }
}

/// Read the exact durable record without following any path component. Missing
/// state is normal; malformed paths and special/oversized files fail closed.
fn read_lifecycle_intent_record(path: &Path) -> Result<Option<Vec<u8>>, RuntimeAvailabilityError> {
    use rustix::fs::{AtFlags, FileType, Mode, OFlags};
    use std::ffi::OsString;
    use std::io::Read as _;

    if !path.is_absolute() {
        return Err(RuntimeAvailabilityError::DurableFilesystem(
            PersistLifecycleIntentError::InvalidDurablePath("path must be absolute"),
        ));
    }
    let mut components = Vec::<OsString>::new();
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(value) => {
                if components.len() == MAX_LIFECYCLE_INTENT_PATH_COMPONENTS {
                    return Err(RuntimeAvailabilityError::DurableFilesystem(
                        PersistLifecycleIntentError::InvalidDurablePath(
                            "path has too many components",
                        ),
                    ));
                }
                components.push(value.to_os_string());
            }
            Component::Prefix(_) | Component::CurDir | Component::ParentDir => {
                return Err(RuntimeAvailabilityError::DurableFilesystem(
                    PersistLifecycleIntentError::InvalidDurablePath(
                        "path contains a non-normal component",
                    ),
                ));
            }
        }
    }
    let file_name = components.pop().ok_or_else(|| {
        RuntimeAvailabilityError::DurableFilesystem(
            PersistLifecycleIntentError::InvalidDurablePath("path has no file name"),
        )
    })?;
    let directory_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut directory = rustix::fs::open("/", directory_flags, Mode::empty()).map_err(|error| {
        RuntimeAvailabilityError::DurableFilesystem(lifecycle_filesystem_error(
            "open filesystem root",
            error,
        ))
    })?;
    for component in components {
        match rustix::fs::statat(&directory, &component, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(metadata) => match FileType::from_raw_mode(metadata.st_mode) {
                FileType::Symlink => {
                    return Err(RuntimeAvailabilityError::DurableFilesystem(
                        PersistLifecycleIntentError::SymlinkRejected,
                    ));
                }
                FileType::Directory => {}
                _ => {
                    return Err(RuntimeAvailabilityError::DurableFilesystem(
                        lifecycle_filesystem_error(
                            "open durable parent",
                            "path component is not a directory",
                        ),
                    ));
                }
            },
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(error) => {
                return Err(RuntimeAvailabilityError::DurableFilesystem(
                    lifecycle_filesystem_error("inspect durable parent", error),
                ));
            }
        }
        directory = rustix::fs::openat(&directory, &component, directory_flags, Mode::empty())
            .map_err(|error| {
                RuntimeAvailabilityError::DurableFilesystem(lifecycle_filesystem_error(
                    "open durable parent",
                    error,
                ))
            })?;
    }
    let metadata = match rustix::fs::statat(&directory, &file_name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(metadata) => metadata,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(error) => {
            return Err(RuntimeAvailabilityError::DurableFilesystem(
                lifecycle_filesystem_error("inspect durable record", error),
            ));
        }
    };
    match FileType::from_raw_mode(metadata.st_mode) {
        FileType::Symlink => {
            return Err(RuntimeAvailabilityError::DurableFilesystem(
                PersistLifecycleIntentError::SymlinkRejected,
            ));
        }
        FileType::RegularFile => {}
        _ => {
            return Err(RuntimeAvailabilityError::DurableFilesystem(
                lifecycle_filesystem_error(
                    "inspect durable record",
                    "target is not a regular file",
                ),
            ));
        }
    }
    let record_size = u64::try_from(metadata.st_size).map_err(|_| {
        RuntimeAvailabilityError::DurableRecord(
            "durable lifecycle record has an invalid negative size".to_string(),
        )
    })?;
    if record_size > MAX_LIFECYCLE_INTENT_RECORD_BYTES as u64 {
        return Err(RuntimeAvailabilityError::DurableRecord(
            "durable lifecycle record exceeds the byte bound".to_string(),
        ));
    }
    let file: std::fs::File = rustix::fs::openat(
        &directory,
        &file_name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| {
        RuntimeAvailabilityError::DurableFilesystem(lifecycle_filesystem_error(
            "open durable record",
            error,
        ))
    })?
    .into();
    let record_capacity = usize::try_from(record_size).map_err(|_| {
        RuntimeAvailabilityError::DurableRecord(
            "durable lifecycle record size cannot be represented".to_string(),
        )
    })?;
    let mut body = Vec::with_capacity(record_capacity);
    file.take((MAX_LIFECYCLE_INTENT_RECORD_BYTES + 1) as u64)
        .read_to_end(&mut body)
        .map_err(|error| {
            RuntimeAvailabilityError::DurableFilesystem(lifecycle_filesystem_error(
                "read durable record",
                error,
            ))
        })?;
    if body.len() > MAX_LIFECYCLE_INTENT_RECORD_BYTES {
        return Err(RuntimeAvailabilityError::DurableRecord(
            "durable lifecycle record grew beyond the byte bound".to_string(),
        ));
    }
    Ok(Some(body))
}

/// Atomically replace one exact absolute record without following symlinks.
fn write_lifecycle_intent_record(
    path: &Path,
    body: &[u8],
) -> Result<(), PersistLifecycleIntentError> {
    use rand::RngCore as _;
    use rustix::fs::{AtFlags, FileType, Mode, OFlags};
    use std::ffi::OsString;
    use std::io::Write as _;

    if !path.is_absolute() {
        return Err(PersistLifecycleIntentError::InvalidDurablePath(
            "path must be absolute",
        ));
    }

    let mut components = Vec::<OsString>::new();
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(value) => {
                if components.len() == MAX_LIFECYCLE_INTENT_PATH_COMPONENTS {
                    return Err(PersistLifecycleIntentError::InvalidDurablePath(
                        "path has too many components",
                    ));
                }
                components.push(value.to_os_string());
            }
            Component::Prefix(_) | Component::CurDir | Component::ParentDir => {
                return Err(PersistLifecycleIntentError::InvalidDurablePath(
                    "path contains a non-normal component",
                ));
            }
        }
    }
    let file_name = components
        .pop()
        .ok_or(PersistLifecycleIntentError::InvalidDurablePath(
            "path has no file name",
        ))?;

    let directory_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut directory = rustix::fs::open("/", directory_flags, Mode::empty())
        .map_err(|error| lifecycle_filesystem_error("open filesystem root", error))?;
    for component in components {
        match rustix::fs::statat(&directory, &component, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(metadata) => match FileType::from_raw_mode(metadata.st_mode) {
                FileType::Symlink => return Err(PersistLifecycleIntentError::SymlinkRejected),
                FileType::Directory => {}
                _ => {
                    return Err(lifecycle_filesystem_error(
                        "open durable parent",
                        "path component is not a directory",
                    ));
                }
            },
            Err(rustix::io::Errno::NOENT) => {
                match rustix::fs::mkdirat(
                    &directory,
                    &component,
                    Mode::RUSR | Mode::WUSR | Mode::XUSR,
                ) {
                    Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                    Err(error) => {
                        return Err(lifecycle_filesystem_error("create durable parent", error));
                    }
                }
            }
            Err(error) => {
                return Err(lifecycle_filesystem_error("inspect durable parent", error));
            }
        }
        directory = rustix::fs::openat(&directory, &component, directory_flags, Mode::empty())
            .map_err(|error| lifecycle_filesystem_error("open durable parent", error))?;
    }

    match rustix::fs::statat(&directory, &file_name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(metadata) => match FileType::from_raw_mode(metadata.st_mode) {
            FileType::Symlink => return Err(PersistLifecycleIntentError::SymlinkRejected),
            FileType::RegularFile => {}
            _ => {
                return Err(lifecycle_filesystem_error(
                    "inspect durable record",
                    "target is not a regular file",
                ));
            }
        },
        Err(rustix::io::Errno::NOENT) => {}
        Err(error) => {
            return Err(lifecycle_filesystem_error("inspect durable record", error));
        }
    }

    let mut random = [0_u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut random);
    let temp_name = format!(
        ".mde-node-availability-{}.tmp",
        random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let file_flags =
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let temp = rustix::fs::openat(
        &directory,
        temp_name.as_str(),
        file_flags,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(|error| lifecycle_filesystem_error("create temporary record", error))?;
    let mut temp_file: std::fs::File = temp.into();
    if let Err(error) = temp_file
        .write_all(body)
        .and_then(|()| temp_file.sync_all())
    {
        drop(temp_file);
        let _ = rustix::fs::unlinkat(&directory, temp_name.as_str(), AtFlags::empty());
        return Err(lifecycle_filesystem_error("write temporary record", error));
    }
    drop(temp_file);

    if let Err(error) = rustix::fs::renameat(
        &directory,
        temp_name.as_str(),
        &directory,
        file_name.as_os_str(),
    ) {
        let _ = rustix::fs::unlinkat(&directory, temp_name.as_str(), AtFlags::empty());
        return Err(lifecycle_filesystem_error("replace durable record", error));
    }
    let directory_file: std::fs::File = directory.into();
    directory_file
        .sync_all()
        .map_err(|error| lifecycle_filesystem_error("sync durable parent", error))
}

/// Why explicit lifecycle evidence was not committed and published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleIntentPublicationError<E> {
    /// Evidence failed shared-policy, replay, transition, or ledger bounds.
    Admission(AvailabilityAdmissionError),
    /// The injected persistence/publication seam did not complete both lanes.
    Output(E),
}

impl<E: fmt::Display> fmt::Display for LifecycleIntentPublicationError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Admission(error) => write!(formatter, "lifecycle intent rejected: {error}"),
            Self::Output(error) => write!(formatter, "lifecycle intent output failed: {error}"),
        }
    }
}

impl<E> std::error::Error for LifecycleIntentPublicationError<E>
where
    E: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Admission(error) => Some(error),
            Self::Output(error) => Some(error),
        }
    }
}

/// Independent roster/heartbeat facts used to evaluate one node.
///
/// `last_seen_at_ms: None` is explicit absence of heartbeat evidence; it is
/// never replaced with an intent timestamp or a fabricated outage time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeAvailabilityEvidence {
    /// Stable node identity matching a ledger intent when one exists.
    pub node_id: String,
    /// Device class selecting the shared governed policy defaults.
    pub device_class: NodeDeviceClass,
    /// Latest independent observation, or `None` when unavailable.
    pub last_seen_at_ms: Option<u64>,
}

/// One deterministic node result from [`AvailabilityLedger::evaluate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeAvailabilityEvaluation {
    /// Stable evaluated node identity.
    pub node_id: String,
    /// Assessment produced by the shared availability policy.
    pub assessment: NodeAvailabilityAssessment,
}

/// Bounded, node-id-sorted output from [`AvailabilityLedger::evaluate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailabilityEvaluationSnapshot {
    /// Evaluations sorted lexicographically by node id.
    pub assessments: Vec<NodeAvailabilityEvaluation>,
}

impl AvailabilityEvaluationSnapshot {
    /// Borrow the sorted evaluations.
    #[must_use]
    pub fn as_slice(&self) -> &[NodeAvailabilityEvaluation] {
        &self.assessments
    }

    /// Look up one evaluated node in the sorted bounded snapshot.
    #[must_use]
    pub fn get(&self, node_id: &str) -> Option<NodeAvailabilityAssessment> {
        self.assessments
            .binary_search_by(|evaluation| evaluation.node_id.as_str().cmp(node_id))
            .ok()
            .map(|index| self.assessments[index].assessment)
    }
}

/// Why a bounded deterministic evaluation could not be produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AvailabilityEvaluationError {
    /// The evidence or ledger/evidence union exceeded the ledger node cap.
    CapacityExceeded {
        /// Maximum distinct nodes accepted by one evaluation.
        capacity: usize,
    },
    /// More than one evidence record named the same node.
    DuplicateEvidence {
        /// Duplicated stable node identity.
        node_id: String,
    },
}

impl fmt::Display for AvailabilityEvaluationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CapacityExceeded { capacity } => {
                write!(
                    formatter,
                    "availability evaluation exceeds node capacity {capacity}"
                )
            }
            Self::DuplicateEvidence { node_id } => {
                write!(
                    formatter,
                    "duplicate availability evidence for node {node_id}"
                )
            }
        }
    }
}

impl std::error::Error for AvailabilityEvaluationError {}

/// A deterministic, latest-intent snapshot of an [`AvailabilityLedger`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailabilitySnapshot {
    /// Current accepted intents sorted by node id.
    pub intents: Vec<NodeAvailabilityIntent>,
}

impl AvailabilitySnapshot {
    /// Borrow the sorted current intents.
    #[must_use]
    pub fn as_slice(&self) -> &[NodeAvailabilityIntent] {
        &self.intents
    }

    /// Consume the snapshot and return its sorted current intents.
    #[must_use]
    pub fn into_intents(self) -> Vec<NodeAvailabilityIntent> {
        self.intents
    }
}

/// Result metadata for one successful ledger admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionReceipt {
    /// Node whose current intent was accepted.
    pub node_id: String,
    /// Accepted producer generation.
    pub generation: u64,
    /// Accepted lifecycle state, including explicit `Unknown`.
    pub state: NodeAvailabilityState,
    /// Whether this admission replaced an existing node entry.
    pub replaced: bool,
}

/// Why a current intent was not admitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AvailabilityAdmissionError {
    /// The shared health contract rejected the record or transition.
    Contract(NodeAvailabilityValidationError),
    /// The record is valid but would add a node beyond the fixed ledger cap.
    CapacityExceeded {
        /// Maximum distinct nodes retained by this ledger.
        capacity: usize,
        /// Node that could not be inserted.
        node_id: String,
    },
}

impl fmt::Display for AvailabilityAdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(error) => write!(formatter, "availability contract rejected: {error}"),
            Self::CapacityExceeded { capacity, node_id } => write!(
                formatter,
                "availability ledger capacity {capacity} exceeded by node {node_id}"
            ),
        }
    }
}

impl std::error::Error for AvailabilityAdmissionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Contract(error) => Some(error),
            Self::CapacityExceeded { .. } => None,
        }
    }
}

/// Why a deterministic fold could not produce a ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AvailabilityFoldError {
    /// The input exceeded the bounded fold budget.
    TooManyEvents {
        /// Maximum records accepted by one fold.
        capacity: usize,
    },
    /// One input failed normal ledger admission.
    Admission(AvailabilityAdmissionError),
}

impl fmt::Display for AvailabilityFoldError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyEvents { capacity } => {
                write!(
                    formatter,
                    "availability fold exceeds event capacity {capacity}"
                )
            }
            Self::Admission(error) => {
                write!(formatter, "availability fold admission failed: {error}")
            }
        }
    }
}

impl std::error::Error for AvailabilityFoldError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::TooManyEvents { .. } => None,
            Self::Admission(error) => Some(error),
        }
    }
}

impl From<AvailabilityAdmissionError> for AvailabilityFoldError {
    fn from(error: AvailabilityAdmissionError) -> Self {
        Self::Admission(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::health::{
        ExpectedReturn, NodeAddressFamily, NodeConnectionType, NodeConnectivitySummary,
        NodeDeviceClass, NODE_AVAILABILITY_INTENT_SCHEMA_VERSION,
    };

    const NOW_MS: u64 = 10_000;

    fn intent(
        node_id: &str,
        state: NodeAvailabilityState,
        generation: u64,
        event_id: &str,
        observed_at_ms: u64,
    ) -> NodeAvailabilityIntent {
        NodeAvailabilityIntent {
            schema_version: NODE_AVAILABILITY_INTENT_SCHEMA_VERSION,
            node_id: node_id.to_string(),
            device_id: format!("{node_id}-device"),
            device_class: NodeDeviceClass::Laptop,
            connection_type: NodeConnectionType::Ethernet,
            state,
            reason: "test intent".to_string(),
            source: "node-availability-test".to_string(),
            event_id: event_id.to_string(),
            generation,
            observed_at_ms,
            expires_at_ms: observed_at_ms + 60_000,
            expected_return: state
                .expects_return()
                .then(|| ExpectedReturn::new(observed_at_ms + 30_000)),
            old_connectivity: None,
            new_connectivity: None,
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestSinkError {
        Unavailable,
    }

    impl fmt::Display for TestSinkError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("test sink unavailable")
        }
    }

    impl std::error::Error for TestSinkError {}

    #[derive(Default)]
    struct RecordingSink {
        calls: usize,
        fail: bool,
        records: Vec<NodeAvailabilityIntent>,
    }

    impl LifecycleIntentSink for RecordingSink {
        type Error = TestSinkError;

        fn persist_and_publish(
            &mut self,
            intent: &NodeAvailabilityIntent,
        ) -> Result<(), Self::Error> {
            self.calls += 1;
            if self.fail {
                return Err(TestSinkError::Unavailable);
            }
            self.records.push(intent.clone());
            Ok(())
        }
    }

    fn adapter_intent(node_id: &str) -> NodeAvailabilityIntent {
        let mut intent = intent(
            node_id,
            NodeAvailabilityState::AdapterMigration,
            2,
            "event-adapter",
            2_000,
        );
        intent.old_connectivity = Some(NodeConnectivitySummary {
            connection_type: NodeConnectionType::Ethernet,
            interface_id: Some("eth0".to_string()),
            address_family: NodeAddressFamily::Ipv4,
            reachable: true,
        });
        intent.new_connectivity = Some(NodeConnectivitySummary {
            connection_type: NodeConnectionType::Wifi,
            interface_id: Some("wlan0".to_string()),
            address_family: NodeAddressFamily::Ipv4,
            reachable: true,
        });
        intent
    }

    fn awake_ledger() -> (AvailabilityLedger<2>, NodeAvailabilityIntent) {
        let awake = intent(
            "node-a",
            NodeAvailabilityState::Awake,
            1,
            "event-awake",
            1_000,
        );
        let mut ledger = AvailabilityLedger::<2>::new();
        ledger.admit(awake.clone(), NOW_MS).expect("seed awake");
        (ledger, awake)
    }

    fn sleep_intent() -> NodeAvailabilityIntent {
        intent(
            "node-a",
            NodeAvailabilityState::Sleeping,
            2,
            "event-sleep",
            2_000,
        )
    }

    #[test]
    fn production_lifecycle_sink_retains_and_publishes_identical_atomic_bytes() {
        use std::io::Read as _;

        let bus_root = tempfile::tempdir().expect("bus root");
        let state_root = tempfile::tempdir().expect("state root");
        let durable_path = state_root.path().join("health/node-a-intent.json");
        let mut persist = Persist::open(bus_root.path().to_path_buf()).expect("open Bus");
        let (mut ledger, _) = awake_ledger();
        let sleeping = sleep_intent();
        let sleeping_body = serde_json::to_vec(&sleeping).expect("encode sleep");

        {
            let mut sink =
                PersistLifecycleIntentSink::new(&mut persist, &durable_path, bus_root.path())
                    .expect("bind Bus generation");
            assert_eq!(sink.durable_path(), durable_path.as_path());
            ledger
                .publish_lifecycle_intent(
                    LifecycleIntentEvidence::Sleep(sleeping.clone()),
                    NOW_MS,
                    &mut sink,
                )
                .expect("persist and publish sleep");
        }

        let first_path_body = std::fs::read(&durable_path).expect("read durable sleep");
        assert_eq!(first_path_body, sleeping_body);
        assert!(first_path_body.len() <= MAX_LIFECYCLE_INTENT_RECORD_BYTES);
        let topic = node_health_topic("node-a");
        assert!(topic.len() <= MAX_LIFECYCLE_INTENT_TOPIC_BYTES);
        let first_rows = persist.list_since(&topic, None).expect("read Bus sleep");
        assert_eq!(first_rows.len(), 1);
        assert_eq!(
            first_rows[0].body.as_deref(),
            std::str::from_utf8(&sleeping_body).ok()
        );

        let mut pre_replace_handle = std::fs::File::open(&durable_path).expect("open old record");
        let returned = intent(
            "node-a",
            NodeAvailabilityState::Returned,
            3,
            "event-returned",
            3_000,
        );
        let returned_body = serde_json::to_vec(&returned).expect("encode returned");
        {
            let mut sink =
                PersistLifecycleIntentSink::new(&mut persist, &durable_path, bus_root.path())
                    .expect("bind Bus generation");
            ledger
                .publish_lifecycle_intent(
                    LifecycleIntentEvidence::Returned(returned.clone()),
                    NOW_MS,
                    &mut sink,
                )
                .expect("atomically replace and publish returned");
        }

        let mut old_open_body = Vec::new();
        pre_replace_handle
            .read_to_end(&mut old_open_body)
            .expect("read pre-replace inode");
        assert_eq!(old_open_body, sleeping_body);
        assert_eq!(
            std::fs::read(&durable_path).expect("read replacement"),
            returned_body
        );
        let rows = persist.list_since(&topic, None).expect("read Bus events");
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0].body.as_deref(),
            std::str::from_utf8(&sleeping_body).ok()
        );
        assert_eq!(
            rows[1].body.as_deref(),
            std::str::from_utf8(&returned_body).ok()
        );
        assert!(durable_path
            .parent()
            .expect("durable parent")
            .read_dir()
            .expect("read durable parent")
            .all(|entry| entry
                .expect("directory entry")
                .file_name()
                .to_string_lossy()
                == "node-a-intent.json"));
    }

    #[test]
    fn production_lifecycle_sink_filesystem_failure_is_explicit_and_precedes_bus() {
        let bus_root = tempfile::tempdir().expect("bus root");
        let state_root = tempfile::tempdir().expect("state root");
        let blocked_parent = state_root.path().join("not-a-directory");
        std::fs::write(&blocked_parent, b"block parent").expect("create blocking file");
        let durable_path = blocked_parent.join("node-a-intent.json");
        let mut persist = Persist::open(bus_root.path().to_path_buf()).expect("open Bus");
        let (mut ledger, awake) = awake_ledger();
        let sleeping = sleep_intent();

        let result = {
            let mut sink =
                PersistLifecycleIntentSink::new(&mut persist, &durable_path, bus_root.path())
                    .expect("bind Bus generation");
            ledger.publish_lifecycle_intent(
                LifecycleIntentEvidence::Sleep(sleeping),
                NOW_MS,
                &mut sink,
            )
        };
        assert!(matches!(
            result,
            Err(LifecycleIntentPublicationError::Output(
                PersistLifecycleIntentError::Filesystem {
                    operation: "open durable parent",
                    ..
                }
            ))
        ));
        assert_eq!(ledger.current("node-a"), Some(&awake));
        assert!(persist
            .read_latest(&node_health_topic("node-a"))
            .expect("read Bus")
            .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn production_lifecycle_sink_rejects_final_and_parent_symlinks() {
        use std::os::unix::fs::symlink;

        let bus_root = tempfile::tempdir().expect("bus root");
        let state_root = tempfile::tempdir().expect("state root");
        let victim = state_root.path().join("victim.json");
        std::fs::write(&victim, b"do not replace").expect("write victim");
        let durable_path = state_root.path().join("node-a-intent.json");
        symlink(&victim, &durable_path).expect("create final symlink");
        let mut persist = Persist::open(bus_root.path().to_path_buf()).expect("open Bus");
        let (mut ledger, awake) = awake_ledger();
        let sleeping = sleep_intent();

        let final_result = {
            let mut sink =
                PersistLifecycleIntentSink::new(&mut persist, &durable_path, bus_root.path())
                    .expect("bind Bus generation");
            ledger.publish_lifecycle_intent(
                LifecycleIntentEvidence::Sleep(sleeping.clone()),
                NOW_MS,
                &mut sink,
            )
        };
        assert!(matches!(
            final_result,
            Err(LifecycleIntentPublicationError::Output(
                PersistLifecycleIntentError::SymlinkRejected
            ))
        ));
        assert_eq!(
            std::fs::read(&victim).expect("read victim"),
            b"do not replace"
        );

        std::fs::remove_file(&durable_path).expect("remove final symlink");
        let real_parent = state_root.path().join("real-parent");
        std::fs::create_dir(&real_parent).expect("create real parent");
        let linked_parent = state_root.path().join("linked-parent");
        symlink(&real_parent, &linked_parent).expect("create parent symlink");
        let through_parent = linked_parent.join("node-a-intent.json");
        let parent_result = {
            let mut sink =
                PersistLifecycleIntentSink::new(&mut persist, &through_parent, bus_root.path())
                    .expect("bind Bus generation");
            ledger.publish_lifecycle_intent(
                LifecycleIntentEvidence::Sleep(sleeping),
                NOW_MS,
                &mut sink,
            )
        };
        assert!(matches!(
            parent_result,
            Err(LifecycleIntentPublicationError::Output(
                PersistLifecycleIntentError::SymlinkRejected
            ))
        ));
        assert_eq!(ledger.current("node-a"), Some(&awake));
        assert!(persist
            .read_latest(&node_health_topic("node-a"))
            .expect("read Bus")
            .is_none());
        assert!(real_parent
            .read_dir()
            .expect("read real parent")
            .next()
            .is_none());
    }

    #[test]
    fn production_lifecycle_sink_bus_failure_keeps_retryable_record_without_commit() {
        let bus_root = tempfile::tempdir().expect("bus root");
        let state_root = tempfile::tempdir().expect("state root");
        let durable_path = state_root.path().join("node-a-intent.json");
        let mut persist = Persist::open(bus_root.path().to_path_buf()).expect("open Bus");
        let bus_blocker = bus_root.path().join("state");
        std::fs::write(&bus_blocker, b"block canonical topic").expect("block Bus topic");
        let (mut ledger, awake) = awake_ledger();
        let sleeping = sleep_intent();
        let expected_body = serde_json::to_vec(&sleeping).expect("encode expected record");

        let failed = {
            let mut sink =
                PersistLifecycleIntentSink::new(&mut persist, &durable_path, bus_root.path())
                    .expect("bind Bus generation");
            ledger.publish_lifecycle_intent(
                LifecycleIntentEvidence::Sleep(sleeping.clone()),
                NOW_MS,
                &mut sink,
            )
        };
        assert!(matches!(
            failed,
            Err(LifecycleIntentPublicationError::Output(
                PersistLifecycleIntentError::Bus(PersistError::Io(_))
            ))
        ));
        assert_eq!(ledger.current("node-a"), Some(&awake));
        assert_eq!(
            std::fs::read(&durable_path).expect("read retryable durable record"),
            expected_body
        );

        std::fs::remove_file(&bus_blocker).expect("unblock Bus topic");
        {
            let mut sink =
                PersistLifecycleIntentSink::new(&mut persist, &durable_path, bus_root.path())
                    .expect("bind Bus generation");
            ledger
                .publish_lifecycle_intent(
                    LifecycleIntentEvidence::Sleep(sleeping.clone()),
                    NOW_MS,
                    &mut sink,
                )
                .expect("retry exact event after output failure");
        }
        assert_eq!(ledger.current("node-a"), Some(&sleeping));
        let rows = persist
            .list_since(&node_health_topic("node-a"), None)
            .expect("read retried Bus event");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].body.as_deref(),
            std::str::from_utf8(&expected_body).ok()
        );
    }

    fn runtime_publisher(
        bus_root: &std::path::Path,
        durable_path: &std::path::Path,
    ) -> RuntimeAvailabilityPublisher {
        RuntimeAvailabilityPublisher::new(
            "node-a".to_string(),
            "node-a-device".to_string(),
            NodeDeviceClass::Laptop,
            bus_root.to_path_buf(),
            durable_path.to_path_buf(),
        )
    }

    fn runtime_sleep_request() -> RuntimeAvailabilityRequest {
        RuntimeAvailabilityRequest::lifecycle(
            NodeAvailabilityState::Sleeping,
            "host-state-power",
            "managed suspend",
            Some(Duration::from_secs(60)),
        )
    }

    fn runtime_returned_request() -> RuntimeAvailabilityRequest {
        RuntimeAvailabilityRequest::lifecycle(
            NodeAvailabilityState::Returned,
            "host-state-power",
            "resume stabilized",
            None,
        )
    }

    #[test]
    fn runtime_publisher_keeps_one_monotonic_generation_across_callers() {
        let bus = tempfile::tempdir().expect("bus");
        let state = tempfile::tempdir().expect("state");
        let durable = state.path().join("availability/current.json");
        let publisher = runtime_publisher(bus.path(), &durable);

        let sleep = publisher
            .publish_at(runtime_sleep_request(), 10_000)
            .expect("publish sleep");
        let duplicate = publisher
            .publish_at(runtime_sleep_request(), 11_000)
            .expect("idempotent caller retry");
        assert_eq!(duplicate.event_id, sleep.event_id);
        assert_eq!(duplicate.generation, 1);

        let returned = publisher
            .publish_at(
                RuntimeAvailabilityRequest::lifecycle(
                    NodeAvailabilityState::Returned,
                    "host-state-power",
                    "resume stabilized",
                    None,
                ),
                12_000,
            )
            .expect("publish returned");
        assert_eq!(returned.generation, 2);
        assert_ne!(returned.event_id, sleep.event_id);

        let persist = Persist::open(bus.path().to_path_buf()).expect("open Bus");
        let rows = persist
            .list_since(&node_health_topic("node-a"), None)
            .expect("read events");
        assert_eq!(rows.len(), 2, "the idempotent retry must not mint an event");
    }

    #[test]
    fn runtime_publisher_corrects_durable_only_event_forward_before_new_work() {
        let bus = tempfile::tempdir().expect("bus");
        let state = tempfile::tempdir().expect("state");
        let durable = state.path().join("availability/current.json");
        let publisher = runtime_publisher(bus.path(), &durable);
        let request = runtime_sleep_request();
        let durable_only = build_runtime_intent(
            "node-a",
            "node-a-device",
            NodeDeviceClass::Laptop,
            None,
            request.clone(),
            20_000,
        )
        .expect("build durable-only event");
        write_lifecycle_intent_record(
            &durable,
            serde_json::to_string(&durable_only).unwrap().as_bytes(),
        )
        .expect("stage interrupted sink result");

        let retried = publisher
            .publish_at(request, 21_000)
            .expect("correct exact event forward");
        assert_eq!(retried, durable_only);
        let persist = Persist::open(bus.path().to_path_buf()).expect("open Bus");
        let rows = persist
            .list_since(&node_health_topic("node-a"), None)
            .expect("read corrected event");
        assert_eq!(rows.len(), 1);
        let expected = serde_json::to_string(&durable_only).unwrap();
        assert_eq!(rows[0].body.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn bus_transaction_recovers_late_storage_with_same_long_running_owner() {
        let holder = tempfile::tempdir().expect("holder");
        let blocked_parent = holder.path().join("blocked-parent");
        std::fs::write(&blocked_parent, b"not a directory").expect("block Bus parent");
        let bus = blocked_parent.join("bus");
        let state = tempfile::tempdir().expect("state");
        let durable = state.path().join("availability/current.json");
        let publisher = runtime_publisher(&bus, &durable);

        assert!(publisher
            .publish_at(runtime_sleep_request(), 40_000)
            .is_err());
        assert!(!durable.exists(), "failed input staging cannot mint truth");

        std::fs::remove_file(&blocked_parent).expect("remove blocker");
        std::fs::create_dir(&blocked_parent).expect("create Bus parent");
        let sleep = publisher
            .publish_at(runtime_sleep_request(), 41_000)
            .expect("the same owner consumes the late Bus");
        assert_eq!(sleep.generation, 1);
        let persist = Persist::open(bus).expect("late Bus");
        assert_eq!(
            persist
                .list_since(&node_health_topic("node-a"), None)
                .expect("late projection")
                .len(),
            1
        );
    }

    #[test]
    fn bus_transaction_unreadable_replacement_preserves_truth_then_corrects_forward() {
        let holder = tempfile::tempdir().expect("holder");
        let bus = holder.path().join("bus");
        let retired = holder.path().join("retired");
        let state = tempfile::tempdir().expect("state");
        let durable = state.path().join("availability/current.json");
        let publisher = runtime_publisher(&bus, &durable);
        let sleep = publisher
            .publish_at(runtime_sleep_request(), 50_000)
            .expect("initial sleep");

        std::fs::rename(&bus, &retired).expect("retire initial Bus");
        std::fs::create_dir(&bus).expect("replacement root");
        std::fs::create_dir(bus.join("index.sqlite")).expect("unreadable index shape");
        assert!(matches!(
            publisher.publish_at(runtime_returned_request(), 51_000),
            Err(RuntimeAvailabilityError::BusIdentity(_))
        ));
        assert_eq!(
            publisher.current_intent().expect("retained truth"),
            Some(sleep.clone())
        );

        std::fs::remove_dir_all(&bus).expect("remove unreadable replacement");
        let replacement = Persist::open(bus.clone()).expect("readable replacement");
        let returned = publisher
            .publish_at(runtime_returned_request(), 52_000)
            .expect("correct retained truth and forward state");
        assert_eq!(returned.generation, 2);
        let rows = replacement
            .list_since(&node_health_topic("node-a"), None)
            .expect("replacement projection");
        assert_eq!(rows.len(), 2, "sleep outbox precedes returned state");
        let projected = rows
            .iter()
            .map(|row| {
                serde_json::from_str::<NodeAvailabilityIntent>(
                    row.body.as_deref().expect("projection body"),
                )
                .expect("typed projection")
            })
            .collect::<Vec<_>>();
        assert_eq!(projected, vec![sleep, returned]);
    }

    #[test]
    fn bus_transaction_same_path_replacement_reaches_long_running_worker_without_restart() {
        let holder = tempfile::tempdir().expect("holder");
        let bus = holder.path().join("bus");
        let retired = holder.path().join("retired");
        let state = tempfile::tempdir().expect("state");
        let durable = state.path().join("availability/current.json");
        let publisher = runtime_publisher(&bus, &durable);
        let (request_tx, request_rx) = std::sync::mpsc::channel();
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            while let Ok((request, now_ms)) = request_rx.recv() {
                if result_tx
                    .send(publisher.publish_at(request, now_ms))
                    .is_err()
                {
                    break;
                }
            }
        });
        request_tx
            .send((runtime_sleep_request(), 60_000))
            .expect("queue initial sleep");
        let sleep = result_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("worker sleep reply")
            .expect("initial sleep");

        std::fs::rename(&bus, &retired).expect("retire Bus at the same path");
        let replacement = Persist::open(bus.clone()).expect("replacement Bus");
        request_tx
            .send((runtime_returned_request(), 61_000))
            .expect("queue forward return");
        let returned = result_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("worker forward reply")
            .expect("same long-running worker corrects and publishes forward");
        assert!(
            !worker.is_finished(),
            "worker must remain live after recovery"
        );

        let replacement_rows = replacement
            .list_since(&node_health_topic("node-a"), None)
            .expect("replacement rows");
        assert_eq!(replacement_rows.len(), 2);
        let replacement_generations = replacement_rows
            .iter()
            .map(|row| {
                serde_json::from_str::<NodeAvailabilityIntent>(row.body.as_deref().expect("body"))
                    .expect("intent")
                    .generation
            })
            .collect::<Vec<_>>();
        assert_eq!(
            replacement_generations,
            vec![sleep.generation, returned.generation]
        );
        let retired = Persist::open(retired).expect("retired Bus");
        assert_eq!(
            retired
                .list_since(&node_health_topic("node-a"), None)
                .expect("retired rows")
                .len(),
            1,
            "forward generation must not leak back into the retired Bus"
        );
        drop(request_tx);
        worker.join().expect("clean worker shutdown");
    }

    #[test]
    fn bus_transaction_replacement_after_write_does_not_commit_and_outbox_recovers() {
        let holder = tempfile::tempdir().expect("holder");
        let bus = holder.path().join("bus");
        let replacement_root = holder.path().join("replacement");
        let retired_root = holder.path().join("retired");
        let state = tempfile::tempdir().expect("state");
        let durable = state.path().join("availability/current.json");
        let mut persist = Persist::open(bus.clone()).expect("initial Bus");
        drop(Persist::open(replacement_root.clone()).expect("replacement Bus"));
        let (mut ledger, awake) = awake_ledger();
        let sleeping = sleep_intent();
        let failed = {
            let mut sink = PersistLifecycleIntentSink::new(&mut persist, &durable, &bus)
                .expect("bind initial Bus")
                .with_replacement_after_write(replacement_root, retired_root.clone());
            ledger.publish_lifecycle_intent(
                LifecycleIntentEvidence::Sleep(sleeping.clone()),
                NOW_MS,
                &mut sink,
            )
        };
        assert!(matches!(
            failed,
            Err(LifecycleIntentPublicationError::Output(
                PersistLifecycleIntentError::BusIdentity(_)
            ))
        ));
        assert_eq!(ledger.current("node-a"), Some(&awake));
        assert_eq!(
            std::fs::read(&durable).expect("durable outbox"),
            serde_json::to_vec(&sleeping).expect("sleep bytes")
        );
        let current = Persist::open(bus.clone()).expect("current Bus");
        assert!(current
            .read_latest(&node_health_topic("node-a"))
            .expect("current projection")
            .is_none());

        let publisher = runtime_publisher(&bus, &durable);
        assert_eq!(
            publisher.correct_forward().expect("recover outbox"),
            Some(sleeping.clone())
        );
        let recovered: NodeAvailabilityIntent = serde_json::from_str(
            current
                .read_latest(&node_health_topic("node-a"))
                .expect("recovered projection")
                .and_then(|message| message.body)
                .as_deref()
                .expect("recovered body"),
        )
        .expect("recovered typed intent");
        assert_eq!(recovered, sleeping);
        assert_eq!(
            Persist::open(retired_root)
                .expect("retired Bus")
                .list_since(&node_health_topic("node-a"), None)
                .expect("retired projection")
                .len(),
            1
        );
    }

    #[test]
    fn runtime_publisher_enforces_exact_shared_reason_bound_before_output() {
        let bus = tempfile::tempdir().expect("bus");
        let state = tempfile::tempdir().expect("state");
        let durable = state.path().join("availability/current.json");
        let publisher = runtime_publisher(bus.path(), &durable);
        let error = publisher
            .publish_at(
                RuntimeAvailabilityRequest::lifecycle(
                    NodeAvailabilityState::Sleeping,
                    "host-state-power",
                    "x".repeat(mackes_mesh_types::health::MAX_NODE_AVAILABILITY_REASON_BYTES + 1),
                    Some(Duration::from_secs(60)),
                ),
                30_000,
            )
            .expect_err("over-bound reason must fail closed");
        assert!(matches!(
            error,
            RuntimeAvailabilityError::Contract(NodeAvailabilityValidationError::FieldTooLong(
                "reason"
            ))
        ));
        assert!(!durable.exists());
    }

    #[test]
    fn lifecycle_publication_accepts_exact_explicit_events_and_output_records() {
        let mut ledger = AvailabilityLedger::<8>::new();
        for node_id in [
            "node-sleep",
            "node-shutdown",
            "node-reboot",
            "node-maintenance",
            "node-adapter",
        ] {
            ledger
                .admit(
                    intent(
                        node_id,
                        NodeAvailabilityState::Awake,
                        1,
                        &format!("event-awake-{node_id}"),
                        1_000,
                    ),
                    NOW_MS,
                )
                .expect("seed explicit awake intent");
        }

        let sleep = intent(
            "node-sleep",
            NodeAvailabilityState::Sleeping,
            2,
            "event-sleep",
            2_000,
        );
        let shutdown = intent(
            "node-shutdown",
            NodeAvailabilityState::ShuttingDown,
            2,
            "event-shutdown",
            2_000,
        );
        let reboot = intent(
            "node-reboot",
            NodeAvailabilityState::ScheduledReboot,
            2,
            "event-reboot",
            2_000,
        );
        let maintenance = intent(
            "node-maintenance",
            NodeAvailabilityState::Maintenance,
            2,
            "event-maintenance",
            2_000,
        );
        let adapter = adapter_intent("node-adapter");
        let returned = intent(
            "node-sleep",
            NodeAvailabilityState::Returned,
            3,
            "event-returned",
            3_000,
        );
        let events = vec![
            LifecycleIntentEvidence::Sleep(sleep.clone()),
            LifecycleIntentEvidence::PlannedShutdown(shutdown.clone()),
            LifecycleIntentEvidence::PlannedReboot(reboot.clone()),
            LifecycleIntentEvidence::Maintenance(maintenance.clone()),
            LifecycleIntentEvidence::AdapterTransition(adapter.clone()),
            LifecycleIntentEvidence::Returned(returned.clone()),
        ];
        let expected = vec![sleep, shutdown, reboot, maintenance, adapter, returned];
        let mut sink = RecordingSink::default();

        for event in events {
            ledger
                .publish_lifecycle_intent(event, NOW_MS, &mut sink)
                .expect("publish explicit lifecycle intent");
        }

        assert_eq!(sink.calls, expected.len());
        assert_eq!(sink.records, expected);
        assert_eq!(
            ledger.current("node-sleep").map(|intent| intent.state),
            Some(NodeAvailabilityState::Returned)
        );
    }

    #[test]
    fn lifecycle_publication_rejects_fabricated_stale_replay_and_contradiction() {
        let mut ledger = AvailabilityLedger::<2>::new();
        let awake = intent(
            "node-a",
            NodeAvailabilityState::Awake,
            1,
            "event-awake",
            1_000,
        );
        ledger.admit(awake.clone(), NOW_MS).expect("seed awake");
        let mut sink = RecordingSink::default();

        let fabricated = intent(
            "node-a",
            NodeAvailabilityState::Awake,
            2,
            "event-fabricated",
            2_000,
        );
        assert!(matches!(
            ledger.publish_lifecycle_intent(
                LifecycleIntentEvidence::Sleep(fabricated),
                NOW_MS,
                &mut sink
            ),
            Err(LifecycleIntentPublicationError::Admission(
                AvailabilityAdmissionError::Contract(
                    NodeAvailabilityValidationError::Contradictory(_)
                )
            ))
        ));

        let mut no_expected_return = intent(
            "node-a",
            NodeAvailabilityState::Sleeping,
            2,
            "event-no-return",
            2_000,
        );
        no_expected_return.expected_return = None;
        assert!(matches!(
            ledger.publish_lifecycle_intent(
                LifecycleIntentEvidence::Sleep(no_expected_return),
                NOW_MS,
                &mut sink
            ),
            Err(LifecycleIntentPublicationError::Admission(
                AvailabilityAdmissionError::Contract(
                    NodeAvailabilityValidationError::ExpectedReturnRequired
                )
            ))
        ));

        let stale = intent(
            "node-a",
            NodeAvailabilityState::Sleeping,
            2,
            "event-stale",
            2_000,
        );
        assert!(matches!(
            ledger.publish_lifecycle_intent(
                LifecycleIntentEvidence::Sleep(stale),
                100_000,
                &mut sink
            ),
            Err(LifecycleIntentPublicationError::Admission(
                AvailabilityAdmissionError::Contract(NodeAvailabilityValidationError::Stale)
            ))
        ));

        let sleeping = intent(
            "node-a",
            NodeAvailabilityState::Sleeping,
            2,
            "event-sleep",
            2_000,
        );
        ledger
            .publish_lifecycle_intent(
                LifecycleIntentEvidence::Sleep(sleeping.clone()),
                NOW_MS,
                &mut sink,
            )
            .expect("publish valid sleep");
        assert_eq!(sink.calls, 1);

        assert!(matches!(
            ledger.publish_lifecycle_intent(
                LifecycleIntentEvidence::Sleep(sleeping.clone()),
                NOW_MS,
                &mut sink
            ),
            Err(LifecycleIntentPublicationError::Admission(
                AvailabilityAdmissionError::Contract(NodeAvailabilityValidationError::Replay)
            ))
        ));

        let stale_generation = intent(
            "node-a",
            NodeAvailabilityState::Returned,
            1,
            "event-old-return",
            3_000,
        );
        assert!(matches!(
            ledger.publish_lifecycle_intent(
                LifecycleIntentEvidence::Returned(stale_generation),
                NOW_MS,
                &mut sink
            ),
            Err(LifecycleIntentPublicationError::Admission(
                AvailabilityAdmissionError::Contract(NodeAvailabilityValidationError::Stale)
            ))
        ));

        let contradictory = intent(
            "node-a",
            NodeAvailabilityState::ShutDown,
            3,
            "event-contradictory",
            3_000,
        );
        assert!(matches!(
            ledger.publish_lifecycle_intent(
                LifecycleIntentEvidence::PlannedShutdown(contradictory),
                NOW_MS,
                &mut sink
            ),
            Err(LifecycleIntentPublicationError::Admission(
                AvailabilityAdmissionError::Contract(
                    NodeAvailabilityValidationError::Contradictory(_)
                )
            ))
        ));

        assert_eq!(sink.calls, 1);
        assert_eq!(sink.records, vec![sleeping.clone()]);
        assert_eq!(ledger.current("node-a"), Some(&sleeping));
    }

    #[test]
    fn lifecycle_publication_output_failure_does_not_commit_or_burn_event() {
        let mut ledger = AvailabilityLedger::<2>::new();
        let awake = intent(
            "node-a",
            NodeAvailabilityState::Awake,
            1,
            "event-awake",
            1_000,
        );
        ledger.admit(awake.clone(), NOW_MS).expect("seed awake");
        let sleeping = intent(
            "node-a",
            NodeAvailabilityState::Sleeping,
            2,
            "event-sleep",
            2_000,
        );
        let mut sink = RecordingSink {
            fail: true,
            ..RecordingSink::default()
        };

        assert!(matches!(
            ledger.publish_lifecycle_intent(
                LifecycleIntentEvidence::Sleep(sleeping.clone()),
                NOW_MS,
                &mut sink
            ),
            Err(LifecycleIntentPublicationError::Output(
                TestSinkError::Unavailable
            ))
        ));
        assert_eq!(ledger.current("node-a"), Some(&awake));
        assert!(sink.records.is_empty());

        sink.fail = false;
        ledger
            .publish_lifecycle_intent(
                LifecycleIntentEvidence::Sleep(sleeping.clone()),
                NOW_MS,
                &mut sink,
            )
            .expect("retry exact uncommitted intent");
        assert_eq!(sink.calls, 2);
        assert_eq!(sink.records, vec![sleeping.clone()]);
        assert_eq!(ledger.current("node-a"), Some(&sleeping));
    }

    #[test]
    fn admission_delegates_replay_stale_and_contradictory_checks() {
        let mut ledger = AvailabilityLedger::<4>::new();
        let awake = intent("node-a", NodeAvailabilityState::Awake, 1, "event-1", 1_000);
        ledger.admit(awake.clone(), NOW_MS).expect("initial intent");

        assert!(matches!(
            ledger.admit(awake, NOW_MS),
            Err(AvailabilityAdmissionError::Contract(
                NodeAvailabilityValidationError::Replay
            ))
        ));

        let sleeping = intent(
            "node-a",
            NodeAvailabilityState::Sleeping,
            2,
            "event-2",
            2_000,
        );
        ledger
            .admit(sleeping.clone(), NOW_MS)
            .expect("valid sleep transition");

        let stale = intent(
            "node-a",
            NodeAvailabilityState::Awake,
            1,
            "event-old",
            1_100,
        );
        assert!(matches!(
            ledger.admit(stale, NOW_MS),
            Err(AvailabilityAdmissionError::Contract(
                NodeAvailabilityValidationError::Stale
            ))
        ));

        let mut expired = intent(
            "node-expired",
            NodeAvailabilityState::Unknown,
            1,
            "event-expired",
            1_000,
        );
        expired.expires_at_ms = 2_000;
        assert!(matches!(
            ledger.admit(expired, NOW_MS),
            Err(AvailabilityAdmissionError::Contract(
                NodeAvailabilityValidationError::Stale
            ))
        ));

        let contradictory = intent(
            "node-a",
            NodeAvailabilityState::ShutDown,
            3,
            "event-3",
            3_000,
        );
        assert!(matches!(
            ledger.admit(contradictory, NOW_MS),
            Err(AvailabilityAdmissionError::Contract(
                NodeAvailabilityValidationError::Contradictory(_)
            ))
        ));

        assert_eq!(ledger.current("node-a"), Some(&sleeping));
    }

    #[test]
    fn bounded_capacity_does_not_mutate_on_rejection() {
        let mut ledger = AvailabilityLedger::<1>::new();
        ledger
            .admit(
                intent("node-a", NodeAvailabilityState::Awake, 1, "event-a", 1_000),
                NOW_MS,
            )
            .expect("first node fits");

        assert!(matches!(
            ledger.admit(
                intent(
                    "node-b",
                    NodeAvailabilityState::Unknown,
                    1,
                    "event-b",
                    1_000,
                ),
                NOW_MS,
            ),
            Err(AvailabilityAdmissionError::CapacityExceeded { capacity: 1, .. })
        ));
        assert_eq!(ledger.len(), 1);
        assert!(ledger.current("node-b").is_none());
    }

    #[test]
    fn explicit_unknown_is_retained_and_never_folded_into_absence() {
        let mut ledger = AvailabilityLedger::<2>::new();
        let unknown = intent(
            "node-unknown",
            NodeAvailabilityState::Unknown,
            1,
            "event-u",
            1_000,
        );
        let receipt = ledger
            .admit(unknown.clone(), NOW_MS)
            .expect("unknown is an explicit valid state");

        assert_eq!(receipt.state, NodeAvailabilityState::Unknown);
        assert_eq!(ledger.current("node-unknown"), Some(&unknown));
        assert_eq!(ledger.snapshot().intents, vec![unknown]);
    }

    fn evidence(
        node_id: &str,
        device_class: NodeDeviceClass,
        last_seen_at_ms: Option<u64>,
    ) -> NodeAvailabilityEvidence {
        NodeAvailabilityEvidence {
            node_id: node_id.to_string(),
            device_class,
            last_seen_at_ms,
        }
    }

    #[test]
    fn evaluation_preserves_unknown_without_matching_evidence() {
        let mut ledger = AvailabilityLedger::<2>::new();
        ledger
            .admit(
                intent(
                    "node-a",
                    NodeAvailabilityState::Sleeping,
                    1,
                    "event-sleep",
                    1_000,
                ),
                NOW_MS,
            )
            .expect("sleep intent");

        assert_eq!(
            ledger.assess_node("node-a", NOW_MS, None),
            NodeAvailabilityAssessment::Unknown
        );
        assert_eq!(
            ledger.assess_node(
                "node-a",
                NOW_MS,
                Some(&evidence(
                    "other-node",
                    NodeDeviceClass::Laptop,
                    Some(9_000),
                ))
            ),
            NodeAvailabilityAssessment::Unknown
        );
        assert_eq!(
            ledger.assess_node(
                "node-a",
                NOW_MS,
                Some(&evidence("node-a", NodeDeviceClass::Server, Some(9_000)))
            ),
            NodeAvailabilityAssessment::Unknown
        );
    }

    #[test]
    fn evaluation_uses_shared_policy_for_expected_absence_and_missed_return() {
        let mut ledger = AvailabilityLedger::<2>::new();
        let mut sleeping = intent(
            "node-a",
            NodeAvailabilityState::Sleeping,
            1,
            "event-sleep",
            1_000,
        );
        sleeping.expires_at_ms = 500_000;
        ledger.admit(sleeping, NOW_MS).expect("sleep intent");
        let evidence = evidence("node-a", NodeDeviceClass::Laptop, Some(9_000));

        assert_eq!(
            ledger.assess_node("node-a", 31_000, Some(&evidence)),
            NodeAvailabilityAssessment::ExpectedAbsence
        );
        assert_eq!(
            ledger.assess_node("node-a", 91_000, Some(&evidence)),
            NodeAvailabilityAssessment::WarningMissedReturn
        );
        assert_eq!(
            ledger.assess_node("node-a", 331_000, Some(&evidence)),
            NodeAvailabilityAssessment::CriticalMissedReturn
        );
    }

    #[test]
    fn evaluation_reports_unannounced_outage_only_from_last_seen_evidence() {
        let ledger = AvailabilityLedger::<2>::new();
        let no_timestamp = evidence("node-a", NodeDeviceClass::Desktop, None);
        let observed = evidence("node-a", NodeDeviceClass::Desktop, Some(1_000));

        assert_eq!(
            ledger.assess_node("node-a", 121_000, Some(&no_timestamp)),
            NodeAvailabilityAssessment::Unknown
        );
        assert_eq!(
            ledger.assess_node("node-a", 31_000, Some(&observed)),
            NodeAvailabilityAssessment::WarningUnannounced
        );
        assert_eq!(
            ledger.assess_node("node-a", 121_000, Some(&observed)),
            NodeAvailabilityAssessment::CriticalUnannounced
        );
    }

    #[test]
    fn evaluation_snapshot_is_sorted_order_independent_and_bounded() {
        let mut ledger = AvailabilityLedger::<3>::new();
        ledger
            .admit(
                intent(
                    "node-z",
                    NodeAvailabilityState::Unknown,
                    1,
                    "event-z",
                    1_000,
                ),
                NOW_MS,
            )
            .expect("unknown intent");
        let node_a = evidence("node-a", NodeDeviceClass::Desktop, Some(1_000));
        let node_z = evidence("node-z", NodeDeviceClass::Laptop, Some(9_000));

        let forward = ledger
            .evaluate(vec![node_z.clone(), node_a.clone()], 121_000)
            .expect("forward evaluation");
        let reverse = ledger
            .evaluate(vec![node_a, node_z], 121_000)
            .expect("reverse evaluation");
        assert_eq!(forward, reverse);
        assert_eq!(
            forward
                .as_slice()
                .iter()
                .map(|evaluation| evaluation.node_id.as_str())
                .collect::<Vec<_>>(),
            vec!["node-a", "node-z"]
        );
        assert_eq!(
            forward.get("node-a"),
            Some(NodeAvailabilityAssessment::CriticalUnannounced)
        );
        assert_eq!(
            forward.get("node-z"),
            Some(NodeAvailabilityAssessment::Unknown)
        );

        let too_many = ledger.evaluate(
            vec![
                evidence("node-a", NodeDeviceClass::Desktop, Some(1_000)),
                evidence("node-b", NodeDeviceClass::Desktop, Some(1_000)),
                evidence("node-c", NodeDeviceClass::Desktop, Some(1_000)),
            ],
            NOW_MS,
        );
        assert!(matches!(
            too_many,
            Err(AvailabilityEvaluationError::CapacityExceeded { capacity: 3 })
        ));

        let duplicate = ledger.evaluate(
            vec![
                evidence("node-z", NodeDeviceClass::Laptop, Some(1_000)),
                evidence("node-z", NodeDeviceClass::Laptop, Some(2_000)),
            ],
            NOW_MS,
        );
        assert!(matches!(
            duplicate,
            Err(AvailabilityEvaluationError::DuplicateEvidence { node_id })
                if node_id == "node-z"
        ));
    }

    #[test]
    fn evaluation_rejects_duplicate_at_capacity_before_distinct_overflow() {
        let ledger = AvailabilityLedger::<2>::new();
        let node_a = evidence("node-a", NodeDeviceClass::Desktop, Some(1_000));
        let node_b = evidence("node-b", NodeDeviceClass::Desktop, Some(1_000));

        for duplicate_order in [
            vec![node_a.clone(), node_a.clone(), node_b.clone()],
            vec![node_b.clone(), node_a.clone(), node_a.clone()],
        ] {
            assert!(matches!(
                ledger.evaluate(duplicate_order, NOW_MS),
                Err(AvailabilityEvaluationError::DuplicateEvidence { node_id })
                    if node_id == "node-a"
            ));
        }

        let distinct_overflow = ledger.evaluate(
            vec![
                node_a,
                node_b,
                evidence("node-c", NodeDeviceClass::Desktop, Some(1_000)),
            ],
            NOW_MS,
        );
        assert!(matches!(
            distinct_overflow,
            Err(AvailabilityEvaluationError::CapacityExceeded { capacity: 2 })
        ));
    }

    #[test]
    fn snapshot_and_fold_are_deterministic_for_reordered_input() {
        let awake = intent("node-z", NodeAvailabilityState::Awake, 1, "event-z", 1_000);
        let sleeping = intent(
            "node-z",
            NodeAvailabilityState::Sleeping,
            2,
            "event-z-sleep",
            2_000,
        );
        let unknown = intent(
            "node-a",
            NodeAvailabilityState::Unknown,
            1,
            "event-a",
            1_000,
        );

        let forward = AvailabilityLedger::<4>::fold(
            vec![awake.clone(), sleeping.clone(), unknown.clone()],
            NOW_MS,
        )
        .expect("forward fold");
        let reverse = AvailabilityLedger::<4>::fold(vec![unknown, sleeping, awake], NOW_MS)
            .expect("reverse fold");

        assert_eq!(forward.snapshot(), reverse.snapshot());
        assert_eq!(
            forward
                .snapshot()
                .intents
                .iter()
                .map(|intent| intent.node_id.as_str())
                .collect::<Vec<_>>(),
            vec!["node-a", "node-z"]
        );
        assert_eq!(
            forward.current("node-z").map(|intent| intent.state),
            Some(NodeAvailabilityState::Sleeping)
        );

        let refolded = AvailabilityLedger::<4>::fold_snapshot(&forward.snapshot(), NOW_MS)
            .expect("snapshot refold");
        assert_eq!(refolded.snapshot(), forward.snapshot());
    }

    #[test]
    fn fold_has_a_hard_input_bound() {
        let intents = (0..=DEFAULT_FOLD_EVENT_CAPACITY)
            .map(|index| {
                intent(
                    &format!("node-{index}"),
                    NodeAvailabilityState::Unknown,
                    1,
                    &format!("event-{index}"),
                    1_000,
                )
            })
            .collect::<Vec<_>>();

        assert!(matches!(
            AvailabilityLedger::<DEFAULT_FOLD_EVENT_CAPACITY>::fold(intents, NOW_MS),
            Err(AvailabilityFoldError::TooManyEvents {
                capacity: DEFAULT_FOLD_EVENT_CAPACITY
            })
        ));
    }
}
