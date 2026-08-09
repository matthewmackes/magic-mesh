//! WL-ARCH-010 — the node-local workload operation worker.
//!
//! This is the only worker allowed to consume `action/workload/operation`.
//! It journals and validates a request before calling an injected adapter, then
//! publishes one bounded `state/workloads/<node>` projection.  The production
//! adapter uses only libvirt/virtqemud or Quadlet/systemd; tests use a fake.

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use mackes_mesh_types::workloads::{
    admit_workload_for_backend, reject_duplicate_json_keys, workload_state_topic, HostCapacity,
    WorkloadAdmission, WorkloadAttachmentLease, WorkloadAttachmentProtocol, WorkloadBackend,
    WorkloadOperationAction, WorkloadOperationErrorCode, WorkloadOperationPhase,
    WorkloadOperationReply, WorkloadOperationRequest, WorkloadOperationStatus, WorkloadPowerState,
    WorkloadReadiness, WorkloadRuntimeSignals, WorkloadStateSnapshot, WorkloadStorageCapacity,
    MAX_WORKLOAD_WIRE_BYTES, WORKLOAD_CONTRACT_SCHEMA_VERSION, WORKLOAD_OPERATION_TOPIC,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use sha2::{Digest, Sha256};

use super::cloud::{
    claim_nonce, verify_token, HmacTokenSigner, NullSigner, TokenSigner, TokenVerdict,
    DEFAULT_AUTH_ROOT,
};
use super::proc::{output_with_timeout, status_with_timeout, DEFAULT_CMD_TIMEOUT};
use super::{ShutdownToken, Worker};
use crate::display1_broker::{
    display1_socket_path_at, register_display1_listener, Display1AttachmentServer, Display1Peer,
    DISPLAY1_SOCKET_ROOT,
};
use crate::workload_reconciler::WorkloadOperationLedger;

/// The sole workload action lane.
pub const ACTION_TOPIC: &str = WORKLOAD_OPERATION_TOPIC;
/// The sole per-node workload state lane.
pub const STATE_TOPIC_PREFIX: &str = "state/workloads/";
/// Poll cadence; all slow actuator calls have their own hard timeout.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Capability verb bound into the exact request token.
pub const AUTH_VERB: &str = "workload-operation";
/// Maximum capability lifetime accepted by this worker.
pub const MAX_AUTH_TTL_MS: i64 = 30_000;
/// Adapter retries are deliberately short and bounded; a deadline remains the
/// hard upper bound, so a stuck backend cannot create a restart storm.
const MAX_ADAPTER_ATTEMPTS: u16 = 8;
const MAX_RETRY_BACKOFF_MS: u64 = 30_000;
/// Rootful Quadlet's transient, node-local source directory. Workload
/// operations are executed by the system daemon, so user-scoped Quadlets
/// would be invisible to the sole systemd authority.
const QUADLET_RUNTIME_ROOT: &str = "/run/containers/systemd";
/// Maximum number of retained Workload action messages admitted per poll.
///
/// The cursor advances only through this page, so a delayed worker drains a
/// large retained backlog over bounded ticks instead of materializing or
/// dispatching the whole history in one recovery pass.
const MAX_OPERATION_MESSAGES_PER_TICK: usize = 64;
/// Rootful Podman's managed graphroot. Container Workloads use the dedicated
/// subtree created by the storage worker so admission and execution observe
/// the same filesystem.
const CONTAINER_STORAGE_PATH: &str = "/var/lib/mde-vms/containers";
const VM_STORAGE_PATH: &str = "/var/lib/mde-vms";
const MIGRATION_COMMAND_JOURNAL_DIR: &str = "migration-commands";
const MAX_PENDING_MIGRATION_COMMANDS: usize = 32;
const MAX_MIGRATION_VM_ID_BYTES: usize = 256;
const MAX_MIGRATION_DOMAIN_XML_BYTES: usize = 1024 * 1024;
const MAX_MIGRATION_COMMAND_RECORD_BYTES: u64 = (MAX_MIGRATION_DOMAIN_XML_BYTES + 4096) as u64;

/// Workload placement is intentionally an exact role check, not a rank floor.
/// A Lighthouse is rank 0 today, but accepting every nonzero rank would turn a
/// malformed or future role value into implicit permission to start a VM or
/// container.  The only role that may host a Workload is the pinned
/// Workstation role (including a headless Workstation).
const fn workload_placement_allowed(role_rank: u8) -> bool {
    role_rank == mde_role::Role::Workstation.rank()
}

/// Result of one adapter operation or observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkloadActuatorOutcome {
    /// Next durable phase.
    pub phase: WorkloadOperationPhase,
    /// Observed power state.
    pub power: WorkloadPowerState,
    /// Independent guest/service/display readiness.
    pub readiness: WorkloadReadiness,
    /// Whether the reconciler may retry after the reported reason.
    pub retryable: bool,
    /// Bounded operator-facing reason.
    pub reason: Option<String>,
    /// Bounded operator-facing remediation.
    pub remediation: Option<String>,
    /// A lease is supplied only after the adapter has created a real,
    /// node-local attachment endpoint. The reconciler never fabricates one
    /// from a request id.
    pub attachment: Option<mackes_mesh_types::workloads::WorkloadAttachmentLease>,
}

/// An adapter error carries its retry policy explicitly.  The reconciler must
/// never infer permanence from human-readable stderr: an unavailable image,
/// invalid identity, or unsafe request fails once, while a temporarily busy
/// libvirt/systemd backend receives the bounded retry budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkloadActuatorError {
    /// The request or managed artifact cannot succeed without operator action.
    Permanent(String),
    /// The backend may converge if retried before the operation deadline.
    Retryable(String),
}

impl std::fmt::Display for WorkloadActuatorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Permanent(reason) | Self::Retryable(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for WorkloadActuatorError {}

impl From<String> for WorkloadActuatorError {
    fn from(reason: String) -> Self {
        Self::Retryable(reason)
    }
}

impl From<&str> for WorkloadActuatorError {
    fn from(reason: &str) -> Self {
        Self::Retryable(reason.to_owned())
    }
}

/// Adapter boundary owned by the compute worker.  No shell, GUI, or Bus code
/// may call libvirt/systemd outside this trait.
pub trait WorkloadActuator: Send + Sync {
    /// Apply one already-journaled operation.
    fn apply(
        &self,
        request: &WorkloadOperationRequest,
    ) -> Result<WorkloadActuatorOutcome, WorkloadActuatorError>;

    /// Cancel a previously accepted operation and clean up any backend and
    /// attachment side effects it may already have created. The target
    /// request is immutable and comes from the durable journal.
    fn cancel(
        &self,
        request: &WorkloadOperationRequest,
        status: &WorkloadOperationStatus,
    ) -> Result<WorkloadActuatorOutcome, WorkloadActuatorError>;

    /// Re-observe an in-flight operation after a restart or poll tick.
    fn observe(
        &self,
        request: &WorkloadOperationRequest,
        status: &WorkloadOperationStatus,
    ) -> Result<Option<WorkloadActuatorOutcome>, WorkloadActuatorError>;

    /// Reconcile a terminal attachment after daemon or host recovery.
    ///
    /// The durable Workload operation remains the sole lifecycle authority.
    /// Implementations may recreate only the exact, unexpired lease already
    /// journaled for that generation; stale identity must be revoked rather
    /// than converted into a new attachment capability.
    fn recover_attachment(
        &self,
        _request: &WorkloadOperationRequest,
        _status: &WorkloadOperationStatus,
        _now_ms: u64,
    ) -> Result<Option<WorkloadActuatorOutcome>, WorkloadActuatorError> {
        Ok(None)
    }

    /// Revoke one exact persisted terminal attachment without changing VM or
    /// container lifecycle state. Implementations without attachment resources
    /// may leave this as a no-op.
    fn revoke_attachment(&self, _status: &WorkloadOperationStatus) {}

    /// Reap ephemeral adapter resources whose lease has expired.
    ///
    /// Implementations without node-local resources may leave this as a
    /// no-op. The compute worker calls it after reconciling in-flight work so
    /// a recovered lease can be refreshed before stale resources are removed.
    fn reap_expired(&self, _now_ms: u64) {}

    /// Capture the current libvirt definition before the source is stopped.
    fn migration_capture_definition(&self, _vm_id: &str) -> Result<String, WorkloadActuatorError> {
        Err(WorkloadActuatorError::Permanent(
            "migration definition capture is not supported by this Workload actuator".into(),
        ))
    }

    /// Ask libvirt to stop the source domain gracefully.
    fn migration_request_stop(&self, _vm_id: &str) -> Result<(), WorkloadActuatorError> {
        Err(WorkloadActuatorError::Permanent(
            "migration stop is not supported by this Workload actuator".into(),
        ))
    }

    /// Return whether libvirt observes the domain in its stopped state.
    fn migration_is_stopped(&self, _vm_id: &str) -> Result<bool, WorkloadActuatorError> {
        Err(WorkloadActuatorError::Permanent(
            "migration observation is not supported by this Workload actuator".into(),
        ))
    }

    /// Define the retained XML and start the domain on this node.
    fn migration_define_and_start(
        &self,
        _vm_id: &str,
        _domain_xml: &str,
    ) -> Result<(), WorkloadActuatorError> {
        Err(WorkloadActuatorError::Permanent(
            "migration define/start is not supported by this Workload actuator".into(),
        ))
    }

    /// Remove only the source domain definition after the target commits.
    fn migration_relinquish_definition(&self, _vm_id: &str) -> Result<(), WorkloadActuatorError> {
        Err(WorkloadActuatorError::Permanent(
            "migration definition relinquish is not supported by this Workload actuator".into(),
        ))
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum WorkloadMigrationCommand {
    CaptureDefinition { vm_id: String },
    RequestStop { vm_id: String },
    ObserveStopped { vm_id: String },
    DefineAndStart { vm_id: String, domain_xml: String },
    RelinquishDefinition { vm_id: String },
}

enum WorkloadMigrationReply {
    Definition(String),
    Stopped(bool),
    Complete,
}

struct WorkloadMigrationEnvelope {
    command_id: String,
    command: WorkloadMigrationCommand,
    reply: SyncSender<Result<WorkloadMigrationReply, WorkloadActuatorError>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum WorkloadMigrationJournalPhase {
    Pending,
    Applied,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkloadMigrationJournalRecord {
    schema_version: u8,
    command_id: String,
    phase: WorkloadMigrationJournalPhase,
    command: WorkloadMigrationCommand,
}

struct WorkloadMigrationJournal {
    root: PathBuf,
}

impl WorkloadMigrationCommand {
    fn validate(&self) -> Result<(), String> {
        let (vm_id, domain_xml) = match self {
            Self::CaptureDefinition { vm_id }
            | Self::RequestStop { vm_id }
            | Self::ObserveStopped { vm_id }
            | Self::RelinquishDefinition { vm_id } => (vm_id, None),
            Self::DefineAndStart { vm_id, domain_xml } => (vm_id, Some(domain_xml)),
        };
        if vm_id.trim().is_empty()
            || vm_id.len() > MAX_MIGRATION_VM_ID_BYTES
            || vm_id.contains('\0')
        {
            return Err("migration command has an invalid VM identity".into());
        }
        if let Some(domain_xml) = domain_xml {
            if domain_xml.trim().is_empty() || domain_xml.len() > MAX_MIGRATION_DOMAIN_XML_BYTES {
                return Err("migration command has an invalid retained domain definition".into());
            }
        }
        Ok(())
    }
}

impl WorkloadMigrationJournal {
    fn open(state_root: &Path) -> Result<Self, String> {
        fs::create_dir_all(state_root)
            .map_err(|error| format!("create Workload state root: {error}"))?;
        let root = state_root.join(MIGRATION_COMMAND_JOURNAL_DIR);
        fs::create_dir_all(&root)
            .map_err(|error| format!("create migration command journal: {error}"))?;
        let metadata = fs::symlink_metadata(&root)
            .map_err(|error| format!("inspect migration command journal: {error}"))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("migration command journal is not a regular directory".into());
        }
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("secure migration command journal: {error}"))?;
        Ok(Self { root })
    }

    fn record_path(&self, command_id: &str) -> PathBuf {
        self.root.join(format!("{command_id}.json"))
    }

    fn store(&self, record: &WorkloadMigrationJournalRecord) -> Result<(), String> {
        if record.schema_version != 1
            || record.command_id.is_empty()
            || record.command_id.len() > 96
            || !record
                .command_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
        {
            return Err("migration command journal identity is invalid".into());
        }
        record.command.validate()?;
        let destination = self.record_path(&record.command_id);
        if !destination.exists() {
            let retained = fs::read_dir(&self.root)
                .map_err(|error| format!("list migration command journal: {error}"))?
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .path()
                        .extension()
                        .is_some_and(|value| value == "json")
                })
                .count();
            if retained >= MAX_PENDING_MIGRATION_COMMANDS {
                return Err("migration command journal is at capacity".into());
            }
        }
        let body = serde_json::to_vec(record)
            .map_err(|error| format!("encode migration command journal record: {error}"))?;
        if u64::try_from(body.len()).unwrap_or(u64::MAX) > MAX_MIGRATION_COMMAND_RECORD_BYTES {
            return Err("migration command journal record is oversized".into());
        }
        let temporary =
            self.root
                .join(format!(".{}.{}.tmp", record.command_id, std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| format!("create migration command journal record: {error}"))?;
        if let Err(error) = file.write_all(&body).and_then(|()| file.sync_all()) {
            let _ = fs::remove_file(&temporary);
            return Err(format!("persist migration command journal record: {error}"));
        }
        if let Err(error) = fs::rename(&temporary, &destination) {
            let _ = fs::remove_file(&temporary);
            return Err(format!("commit migration command journal record: {error}"));
        }
        fs::File::open(&self.root)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("sync migration command journal directory: {error}"))?;
        Ok(())
    }

    fn pending(&self) -> Result<Vec<WorkloadMigrationJournalRecord>, String> {
        let mut paths = fs::read_dir(&self.root)
            .map_err(|error| format!("list migration command journal: {error}"))?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|value| value == "json"))
            .collect::<Vec<_>>();
        paths.sort();
        if paths.len() > MAX_PENDING_MIGRATION_COMMANDS {
            return Err("migration command journal exceeds its bounded capacity".into());
        }
        let mut records = Vec::with_capacity(paths.len());
        for path in paths {
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| format!("inspect migration command journal record: {error}"))?;
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() > MAX_MIGRATION_COMMAND_RECORD_BYTES
            {
                return Err("migration command journal contains an unsafe record".into());
            }
            let body = fs::read(&path)
                .map_err(|error| format!("read migration command journal record: {error}"))?;
            let text = std::str::from_utf8(&body)
                .map_err(|_| "migration command journal record is not UTF-8".to_string())?;
            reject_duplicate_json_keys(text)
                .map_err(|_| "migration command journal record has duplicate keys".to_string())?;
            let record: WorkloadMigrationJournalRecord = serde_json::from_slice(&body)
                .map_err(|error| format!("decode migration command journal record: {error}"))?;
            if record.schema_version != 1
                || self.record_path(&record.command_id) != path
                || record.command.validate().is_err()
            {
                return Err("migration command journal record failed validation".into());
            }
            records.push(record);
        }
        Ok(records)
    }

    fn remove(&self, command_id: &str) -> Result<(), String> {
        match fs::remove_file(self.record_path(command_id)) {
            Ok(()) => fs::File::open(&self.root)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| format!("sync migration command journal cleanup: {error}")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("remove migration command journal record: {error}")),
        }
    }
}

static WORKLOAD_MIGRATION_EXECUTOR: OnceLock<Mutex<Option<SyncSender<WorkloadMigrationEnvelope>>>> =
    OnceLock::new();
static WORKLOAD_MIGRATION_COMMAND_SEQUENCE: AtomicU64 = AtomicU64::new(1);

fn migration_executor_registry() -> &'static Mutex<Option<SyncSender<WorkloadMigrationEnvelope>>> {
    WORKLOAD_MIGRATION_EXECUTOR.get_or_init(|| Mutex::new(None))
}

fn next_migration_command_id() -> String {
    let epoch_nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let sequence = WORKLOAD_MIGRATION_COMMAND_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!(
        "{epoch_nanos:032x}-{:08x}-{sequence:016x}",
        std::process::id()
    )
}

/// Command-side handle for cold migration. It has no actuator and cannot
/// execute libvirt; the receiving [`WorkloadComputeWorker`] owns execution.
#[derive(Debug, Clone, Copy, Default)]
pub struct WorkloadMigrationClient;

impl WorkloadMigrationClient {
    fn dispatch(
        self,
        command: WorkloadMigrationCommand,
    ) -> Result<WorkloadMigrationReply, WorkloadActuatorError> {
        command
            .validate()
            .map_err(WorkloadActuatorError::Permanent)?;
        let (reply_tx, reply_rx) = sync_channel(1);
        let mut envelope = WorkloadMigrationEnvelope {
            command_id: next_migration_command_id(),
            command,
            reply: reply_tx,
        };
        let deadline = std::time::Instant::now() + DEFAULT_CMD_TIMEOUT;
        loop {
            let sender = migration_executor_registry()
                .lock()
                .ok()
                .and_then(|slot| slot.clone());
            if let Some(sender) = sender {
                match sender.try_send(envelope) {
                    Ok(()) => {
                        return reply_rx
                            .recv_timeout(DEFAULT_CMD_TIMEOUT)
                            .map_err(|error| {
                                WorkloadActuatorError::Retryable(format!(
                                    "Workload migration reconciler reply unavailable: {error}"
                                ))
                            })?;
                    }
                    Err(TrySendError::Full(returned) | TrySendError::Disconnected(returned)) => {
                        envelope = returned;
                    }
                }
            }
            if std::time::Instant::now() >= deadline {
                return Err(WorkloadActuatorError::Retryable(
                    "Workload migration reconciler is unavailable".into(),
                ));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Ask the reconciler to capture the current domain definition.
    pub fn capture_definition(self, vm_id: &str) -> Result<String, WorkloadActuatorError> {
        match self.dispatch(WorkloadMigrationCommand::CaptureDefinition {
            vm_id: vm_id.to_owned(),
        })? {
            WorkloadMigrationReply::Definition(xml) => Ok(xml),
            _ => Err(WorkloadActuatorError::Permanent(
                "Workload migration reconciler returned the wrong capture reply".into(),
            )),
        }
    }

    /// Ask the reconciler to request graceful source shutdown.
    pub fn request_stop(self, vm_id: &str) -> Result<(), WorkloadActuatorError> {
        match self.dispatch(WorkloadMigrationCommand::RequestStop {
            vm_id: vm_id.to_owned(),
        })? {
            WorkloadMigrationReply::Complete => Ok(()),
            _ => Err(WorkloadActuatorError::Permanent(
                "Workload migration reconciler returned the wrong stop reply".into(),
            )),
        }
    }

    /// Ask the reconciler for the source domain's stopped observation.
    pub fn is_stopped(self, vm_id: &str) -> Result<bool, WorkloadActuatorError> {
        match self.dispatch(WorkloadMigrationCommand::ObserveStopped {
            vm_id: vm_id.to_owned(),
        })? {
            WorkloadMigrationReply::Stopped(stopped) => Ok(stopped),
            _ => Err(WorkloadActuatorError::Permanent(
                "Workload migration reconciler returned the wrong observation reply".into(),
            )),
        }
    }

    /// Ask the reconciler to define and start a retained migration definition.
    pub fn define_and_start(
        self,
        vm_id: &str,
        domain_xml: &str,
    ) -> Result<(), WorkloadActuatorError> {
        match self.dispatch(WorkloadMigrationCommand::DefineAndStart {
            vm_id: vm_id.to_owned(),
            domain_xml: domain_xml.to_owned(),
        })? {
            WorkloadMigrationReply::Complete => Ok(()),
            _ => Err(WorkloadActuatorError::Permanent(
                "Workload migration reconciler returned the wrong define reply".into(),
            )),
        }
    }

    /// Ask the reconciler to relinquish the committed source definition.
    pub fn relinquish_definition(self, vm_id: &str) -> Result<(), WorkloadActuatorError> {
        match self.dispatch(WorkloadMigrationCommand::RelinquishDefinition {
            vm_id: vm_id.to_owned(),
        })? {
            WorkloadMigrationReply::Complete => Ok(()),
            _ => Err(WorkloadActuatorError::Permanent(
                "Workload migration reconciler returned the wrong relinquish reply".into(),
            )),
        }
    }
}

/// Authorization boundary.  Production verifies a short-lived armed token;
/// tests and future local-seat integration inject a deterministic verifier.
pub trait WorkloadAuthorizer: Send + Sync {
    /// Verify and consume the exact-body capability before any journal mutation
    /// or backend side effect.
    fn authorize(
        &self,
        raw_body: &str,
        request: &WorkloadOperationRequest,
        now_ms: i64,
    ) -> Result<(), String>;
}

struct ArmedWorkloadAuthorizer {
    signer: Box<dyn TokenSigner>,
    auth_root: PathBuf,
}

impl WorkloadAuthorizer for ArmedWorkloadAuthorizer {
    fn authorize(
        &self,
        raw_body: &str,
        request: &WorkloadOperationRequest,
        now_ms: i64,
    ) -> Result<(), String> {
        let target = format!("workload:{}", request.workload_id.as_str());
        let verdict = verify_token(
            request.armed_token.as_deref(),
            AUTH_VERB,
            &request.target_node,
            &target,
            raw_body,
            now_ms,
            self.signer.as_ref(),
        );
        if verdict != TokenVerdict::Valid {
            return Err(format!("workload capability rejected: {verdict:?}"));
        }
        let token = request
            .armed_token
            .as_deref()
            .and_then(mackes_mesh_types::cloud::CloudArmedToken::parse)
            .ok_or_else(|| {
                "workload capability was not parseable after verification".to_string()
            })?;
        if token.expires_at_ms > now_ms.saturating_add(MAX_AUTH_TTL_MS) {
            return Err("workload capability lifetime exceeds 30 seconds".to_string());
        }
        match claim_nonce(&self.auth_root, &token.nonce, token.expires_at_ms, now_ms) {
            Ok(true) => Ok(()),
            Ok(false) => Err("workload capability was already used".to_string()),
            Err(error) => Err(format!(
                "workload capability replay store unavailable: {error}"
            )),
        }
    }
}

/// Production libvirt/Quadlet adapter.  Every command is bounded and every
/// identity came through the shared path-safe `WorkloadId` validator.
pub struct SystemWorkloadActuator {
    workgroup_root: PathBuf,
    display1_root: PathBuf,
    attachments: Arc<Mutex<BTreeMap<String, Arc<Display1AttachmentRuntime>>>>,
}

/// Runtime ownership for one authenticated Display1 lease. The server is
/// created before the VM side effect; the QEMU peer is registered only after
/// libvirt reports a running DBus graphics endpoint. Keeping the peer alive is
/// part of the attachment contract—dropping it would silently unregister the
/// listener while the Workload still reports progress.
struct Display1AttachmentRuntime {
    server: Arc<Display1AttachmentServer>,
    peer: Arc<Mutex<Option<Display1Peer>>>,
    registration: Arc<AtomicU8>,
    error: Arc<Mutex<Option<String>>>,
    shutdown: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

const DISPLAY1_REGISTRATION_NEW: u8 = 0;
const DISPLAY1_REGISTRATION_PENDING: u8 = 1;
const DISPLAY1_REGISTRATION_READY: u8 = 2;
const DISPLAY1_REGISTRATION_FAILED: u8 = 3;

impl Display1AttachmentRuntime {
    fn start(
        root: &Path,
        lease: WorkloadAttachmentLease,
    ) -> Result<Arc<Self>, WorkloadActuatorError> {
        let server = Display1AttachmentServer::start_at(root, lease).map_err(|error| {
            WorkloadActuatorError::Retryable(format!("start Display1 broker: {error}"))
        })?;
        Ok(Arc::new(Self {
            server: Arc::new(server),
            peer: Arc::new(Mutex::new(None)),
            registration: Arc::new(AtomicU8::new(DISPLAY1_REGISTRATION_NEW)),
            error: Arc::new(Mutex::new(None)),
            shutdown: Arc::new(AtomicBool::new(false)),
            thread: Mutex::new(None),
        }))
    }

    fn register(&self, qemu_address: String) {
        if self
            .registration
            .compare_exchange(
                DISPLAY1_REGISTRATION_NEW,
                DISPLAY1_REGISTRATION_PENDING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return;
        }
        let sink = self.server.frame_sink();
        let peer = Arc::clone(&self.peer);
        let registration = Arc::clone(&self.registration);
        let error = Arc::clone(&self.error);
        let shutdown = Arc::clone(&self.shutdown);
        let thread = thread::Builder::new()
            .name(format!(
                "display1-register-{}",
                self.server.lease().lease_id
            ))
            .spawn(move || {
                let result = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|runtime| format!("build Display1 runtime: {runtime}"))
                    .and_then(|runtime| {
                        runtime
                            .block_on(tokio::time::timeout(
                                Duration::from_secs(5),
                                register_display1_listener(&qemu_address, sink),
                            ))
                            .map_err(|_| {
                                "QEMU Display1 listener registration timed out".to_string()
                            })?
                            .map_err(|attach| format!("register QEMU Display1 listener: {attach}"))
                    });
                match result {
                    Ok(display1_peer) => {
                        if let Ok(mut slot) = peer.lock() {
                            *slot = Some(display1_peer);
                            registration.store(DISPLAY1_REGISTRATION_READY, Ordering::Release);
                        } else {
                            registration.store(DISPLAY1_REGISTRATION_FAILED, Ordering::Release);
                        }
                        while !shutdown.load(Ordering::Acquire) {
                            thread::sleep(Duration::from_millis(100));
                        }
                    }
                    Err(reason) => {
                        if let Ok(mut slot) = error.lock() {
                            *slot = Some(reason);
                        }
                        registration.store(DISPLAY1_REGISTRATION_FAILED, Ordering::Release);
                    }
                }
            });
        match thread {
            Ok(thread) => {
                if let Ok(mut slot) = self.thread.lock() {
                    *slot = Some(thread);
                } else {
                    self.shutdown.store(true, Ordering::Release);
                    self.registration
                        .store(DISPLAY1_REGISTRATION_FAILED, Ordering::Release);
                }
            }
            Err(error) => {
                if let Ok(mut slot) = self.error.lock() {
                    *slot = Some(format!("spawn Display1 registration: {error}"));
                }
                self.registration
                    .store(DISPLAY1_REGISTRATION_FAILED, Ordering::Release);
            }
        }
    }

    fn registration_state(&self) -> u8 {
        self.registration.load(Ordering::Acquire)
    }

    fn registration_error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|slot| slot.clone())
    }

    fn first_frame_seen(&self) -> bool {
        self.server.first_frame_seen()
    }
}

impl Drop for Display1AttachmentRuntime {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Ok(mut thread) = self.thread.lock() {
            if let Some(thread) = thread.take() {
                let _ = thread.join();
            }
        }
    }
}

impl SystemWorkloadActuator {
    /// Construct the production adapter with the node-local replicated state
    /// root that contains the approved image catalog.
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self {
            workgroup_root,
            display1_root: PathBuf::from(DISPLAY1_SOCKET_ROOT),
            attachments: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    #[cfg(test)]
    fn with_display1_root(mut self, display1_root: PathBuf) -> Self {
        self.display1_root = display1_root;
        self
    }

    fn attachment_lease(
        request: &WorkloadOperationRequest,
        generation: u64,
        now_ms: u64,
    ) -> WorkloadAttachmentLease {
        let mut digest = Sha256::new();
        digest.update(request.workload_id.as_str().as_bytes());
        digest.update([0]);
        digest.update(request.request_id.as_bytes());
        digest.update([0]);
        digest.update(generation.to_be_bytes());
        let digest = hex_digest(&digest.finalize());
        WorkloadAttachmentLease {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            lease_id: format!("display1-{}", &digest[..32]),
            nonce: digest,
            workload_id: request.workload_id.clone(),
            generation: generation.max(1),
            protocol: WorkloadAttachmentProtocol::QemuDisplay1Dmabuf,
            expires_at_ms: request
                .deadline_at_ms
                .min(now_ms.saturating_add(MAX_AUTH_TTL_MS as u64)),
        }
    }

    fn ensure_attachment(
        &self,
        request: &WorkloadOperationRequest,
        generation: u64,
        now_ms: u64,
    ) -> Result<Arc<Display1AttachmentRuntime>, WorkloadActuatorError> {
        let key = request.workload_id.as_str().to_owned();
        let mut attachments = self.attachments.lock().map_err(|_| {
            WorkloadActuatorError::Retryable("Display1 attachment store poisoned".into())
        })?;
        if let Some(runtime) = attachments.get(&key) {
            if runtime.server.lease().generation == generation
                && runtime.server.lease().expires_at_ms > now_ms
            {
                return Ok(Arc::clone(runtime));
            }
            attachments.remove(&key);
        }
        let lease = Self::attachment_lease(request, generation, now_ms);
        lease.validate(now_ms).map_err(|error| {
            WorkloadActuatorError::Permanent(format!("invalid Display1 lease: {error}"))
        })?;
        let runtime = Display1AttachmentRuntime::start(&self.display1_root, lease)?;
        attachments.insert(key, Arc::clone(&runtime));
        Ok(runtime)
    }

    fn attachment_for_status(
        &self,
        request: &WorkloadOperationRequest,
        status: &WorkloadOperationStatus,
        now_ms: u64,
    ) -> Result<Arc<Display1AttachmentRuntime>, WorkloadActuatorError> {
        // Expiration is normal for a durable operation that survived a slow
        // restart. The projection drops the stale descriptor, and recovery
        // must mint a fresh lease for the same generation instead of turning
        // an ephemeral timeout into a permanent operation failure.
        if let Some(lease) = status
            .attachment
            .as_ref()
            .filter(|lease| lease.expires_at_ms > now_ms)
        {
            lease.validate(now_ms).map_err(|error| {
                WorkloadActuatorError::Permanent(format!(
                    "invalid persisted Display1 lease: {error}"
                ))
            })?;
            let key = request.workload_id.as_str().to_owned();
            if let Ok(attachments) = self.attachments.lock() {
                if let Some(runtime) = attachments.get(&key) {
                    if runtime.server.lease() == lease {
                        return Ok(Arc::clone(runtime));
                    }
                }
            }
            let runtime = Display1AttachmentRuntime::start(&self.display1_root, lease.clone())?;
            self.attachments
                .lock()
                .map_err(|_| {
                    WorkloadActuatorError::Retryable("Display1 attachment store poisoned".into())
                })?
                .insert(key, Arc::clone(&runtime));
            return Ok(runtime);
        }
        self.ensure_attachment(request, status.generation, now_ms)
    }

    fn remove_attachment(&self, request: &WorkloadOperationRequest) {
        if let Ok(mut attachments) = self.attachments.lock() {
            attachments.remove(request.workload_id.as_str());
        }
    }

    fn revoke_persisted_attachment(&self, status: &WorkloadOperationStatus) {
        let Some(lease) = status.attachment.as_ref() else {
            return;
        };
        if let Ok(mut attachments) = self.attachments.lock() {
            let workload_id = status.workload_id.as_str();
            if attachments
                .get(workload_id)
                .is_some_and(|runtime| runtime.server.lease() == lease)
            {
                attachments.remove(workload_id);
            }
        }
        let Some(socket) = display1_socket_path_at(&self.display1_root, &lease.lease_id) else {
            return;
        };
        match fs::remove_file(&socket) {
            Ok(()) => {
                tracing::info!(path = %socket.display(), "revoked stale Display1 lease socket")
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(path = %socket.display(), %error, "stale Display1 lease socket could not be removed")
            }
        }
    }

    fn recovered_attachment_unavailable(reason: impl Into<String>) -> WorkloadActuatorOutcome {
        WorkloadActuatorOutcome {
            phase: WorkloadOperationPhase::Completed,
            power: WorkloadPowerState::Running,
            readiness: WorkloadReadiness::Unavailable,
            retryable: false,
            reason: Some(reason.into()),
            remediation: Some(
                "return to Workloads and issue a new Start and attach operation from the current generation"
                    .into(),
            ),
            attachment: None,
        }
    }

    fn stopped_outcome(&self, request: &WorkloadOperationRequest) -> WorkloadActuatorOutcome {
        // A stopped guest cannot consume or acknowledge the authenticated
        // Display1 attachment. Release the actual node-local runtime now;
        // lease expiry is only a crash/restart safety net, not the normal
        // Stop completion path.
        self.remove_attachment(request);
        WorkloadActuatorOutcome {
            phase: WorkloadOperationPhase::Completed,
            power: WorkloadPowerState::Stopped,
            readiness: WorkloadReadiness::Unavailable,
            retryable: false,
            reason: Some("workload is not running".into()),
            remediation: None,
            attachment: None,
        }
    }

    fn qemu_display1_address(
        request: &WorkloadOperationRequest,
    ) -> Result<String, WorkloadActuatorError> {
        let mut command = Command::new("virsh");
        command.args([
            "--connect",
            "qemu:///system",
            "domdisplay",
            "--type",
            "dbus",
            Self::libvirt_domain(request),
        ]);
        let output = output_with_timeout(command, DEFAULT_CMD_TIMEOUT).map_err(|error| {
            WorkloadActuatorError::Retryable(format!("Display1 address probe failed: {error}"))
        })?;
        if !output.status.success() {
            return Err(WorkloadActuatorError::Retryable(
                "libvirt has not published a QEMU Display1 address".into(),
            ));
        }
        normalize_qemu_display1_address(String::from_utf8_lossy(&output.stdout).as_ref())
    }

    fn domain_defined(request: &WorkloadOperationRequest) -> Result<bool, String> {
        let mut command = Command::new("virsh");
        command.args([
            "--connect",
            "qemu:///system",
            "dominfo",
            Self::libvirt_domain(request),
        ]);
        let output = output_with_timeout(command, DEFAULT_CMD_TIMEOUT)
            .map_err(|error| format!("domain existence probe failed: {error}"))?;
        if output.status.success() {
            return Ok(true);
        }
        let detail = String::from_utf8_lossy(&output.stderr);
        if detail.contains("failed to get domain")
            || detail.contains("Domain not found")
            || detail.contains("domain not found")
        {
            Ok(false)
        } else {
            Err(format!(
                "domain existence probe was not authoritative: {}",
                bounded_reason(detail.trim())
            ))
        }
    }

    fn approved_image(&self, image_ref: &str) -> Result<PathBuf, String> {
        let (name, version) = image_ref
            .split_once(':')
            .filter(|(name, version)| !name.is_empty() && !version.is_empty())
            .ok_or_else(|| "image_ref must be an approved name:version reference".to_string())?;
        if name.contains(':')
            || version.contains(':')
            || !name
                .chars()
                .all(|value| value.is_ascii_alphanumeric() || matches!(value, '-' | '_' | '.'))
            || !version
                .chars()
                .all(|value| value.is_ascii_alphanumeric() || matches!(value, '-' | '_' | '.'))
        {
            return Err("image_ref contains an invalid catalog identity".to_string());
        }
        let manifest = crate::image_catalog::load_manifests(&self.workgroup_root)
            .into_iter()
            .find(|manifest| {
                manifest.name == name
                    && manifest.version == version
                    && crate::image_catalog::ImageKind::parse(&manifest.kind)
                        == Some(crate::image_catalog::ImageKind::Vm)
            })
            .ok_or_else(|| format!("approved VM image {name}:{version} is not in the catalog"))?;
        let marker = crate::image_catalog::images_dir(&self.workgroup_root)
            .join(name)
            .join("PROMOTED");
        let promoted = fs::read_to_string(&marker)
            .map_err(|error| format!("read image approval marker: {error}"))?;
        if promoted.trim() != version {
            return Err(format!(
                "VM image {name}:{version} is not the approved promoted version"
            ));
        }
        let artifact = crate::image_catalog::images_dir(&self.workgroup_root)
            .join(&manifest.name)
            .join(&manifest.version)
            .join(format!("{}.img", manifest.name));
        let metadata = fs::metadata(&artifact)
            .map_err(|error| format!("approved VM image artifact is unavailable: {error}"))?;
        if !metadata.is_file() {
            return Err("approved VM image artifact is not a regular file".to_string());
        }
        Ok(artifact)
    }

    /// Resolve a promoted, node-local OCI artifact before handing power to
    /// systemd.  A Quadlet unit name alone is not provenance: without this
    /// check a stale or peer-injected unit could make the Workload API start
    /// an image that never passed the catalog promotion boundary.
    fn approved_container_image(&self, image_ref: &str) -> Result<PathBuf, String> {
        let (name, version) = image_ref
            .split_once(':')
            .filter(|(name, version)| !name.is_empty() && !version.is_empty())
            .ok_or_else(|| "image_ref must be an approved name:version reference".to_string())?;
        if name.contains(':')
            || version.contains(':')
            || !name
                .chars()
                .all(|value| value.is_ascii_alphanumeric() || matches!(value, '-' | '_' | '.'))
            || !version
                .chars()
                .all(|value| value.is_ascii_alphanumeric() || matches!(value, '-' | '_' | '.'))
        {
            return Err("image_ref contains an invalid catalog identity".to_string());
        }
        let manifest = crate::image_catalog::load_manifests(&self.workgroup_root)
            .into_iter()
            .find(|manifest| {
                manifest.name == name
                    && manifest.version == version
                    && crate::image_catalog::ImageKind::parse(&manifest.kind)
                        == Some(crate::image_catalog::ImageKind::Container)
            })
            .ok_or_else(|| {
                format!("approved container image {name}:{version} is not in the catalog")
            })?;
        let marker = crate::image_catalog::images_dir(&self.workgroup_root)
            .join(name)
            .join("PROMOTED");
        let promoted = fs::read_to_string(&marker)
            .map_err(|error| format!("read container approval marker: {error}"))?;
        if promoted.trim() != version {
            return Err(format!(
                "container image {name}:{version} is not the approved promoted version"
            ));
        }
        let artifact = crate::image_catalog::images_dir(&self.workgroup_root)
            .join(&manifest.name)
            .join(&manifest.version)
            .join(format!("{}-{}.oci.tar", manifest.name, manifest.version));
        let metadata = fs::symlink_metadata(&artifact).map_err(|error| {
            format!("approved container image artifact is unavailable: {error}")
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("approved container image artifact is not a regular file".to_string());
        }
        if metadata.len() == 0 {
            return Err("approved container image artifact is empty".to_string());
        }
        Ok(artifact)
    }

    fn define_vm(&self, request: &WorkloadOperationRequest) -> Result<(), WorkloadActuatorError> {
        if request.backend != WorkloadBackend::LibvirtVirtqemud {
            return Ok(());
        }
        if Self::domain_defined(request)? {
            return Ok(());
        }
        let image_ref = request.image_ref.as_deref().ok_or_else(|| {
            WorkloadActuatorError::Permanent(
                "StartAndAttach cannot define a VM without an approved image_ref".to_string(),
            )
        })?;
        let base = self
            .approved_image(image_ref)
            .map_err(WorkloadActuatorError::Permanent)?;
        let pool = Path::new("/var/lib/mde-vms");
        let pool_metadata = fs::metadata(pool).map_err(|error| {
            WorkloadActuatorError::Retryable(format!("mde-vms pool is unavailable: {error}"))
        })?;
        if !pool_metadata.is_dir() {
            return Err(WorkloadActuatorError::Permanent(
                "mde-vms pool path is not a directory".to_string(),
            ));
        }
        let domain = Self::libvirt_domain(request);
        let disk = pool.join(format!("{domain}.qcow2"));
        let disk_string = disk.to_string_lossy().into_owned();
        let image_args = crate::workers::workload_vm::build_qemu_img_argv(
            Some(&base.to_string_lossy()),
            &disk_string,
            u64::from(request.resources.disk_gb),
        );
        let mut image_command = Command::new("qemu-img");
        image_command.args(&image_args);
        let image_status =
            status_with_timeout(image_command, DEFAULT_CMD_TIMEOUT).map_err(|error| {
                WorkloadActuatorError::Retryable(format!(
                    "VM overlay creation failed to start: {error}"
                ))
            })?;
        if !image_status.success() {
            return Err(WorkloadActuatorError::Retryable(format!(
                "VM overlay creation exited with {image_status}"
            )));
        }

        let spec = crate::workers::workload_vm::VmDomainSpec {
            name: domain.to_string(),
            vcpus: u32::from(request.resources.vcpu),
            ram_mb: u64::from(request.resources.memory_mb),
            network: Some("default".to_string()),
        };
        let xml = crate::workers::workload_vm::build_domain_xml(&spec, &disk_string);
        let xml_path = std::env::temp_dir().join(format!("mde-workload-{domain}.xml"));
        fs::write(&xml_path, xml.as_bytes()).map_err(|error| {
            WorkloadActuatorError::Retryable(format!("write VM definition: {error}"))
        })?;
        let mut define_command = Command::new("virsh");
        define_command.args([
            "--connect",
            "qemu:///system",
            "define",
            &xml_path.to_string_lossy(),
        ]);
        let define_result =
            status_with_timeout(define_command, DEFAULT_CMD_TIMEOUT).map_err(|error| {
                WorkloadActuatorError::Retryable(format!("VM definition failed to start: {error}"))
            });
        let _ = fs::remove_file(&xml_path);
        let define_status = match define_result {
            Ok(status) => status,
            Err(error) => {
                let _ = fs::remove_file(&disk);
                return Err(error);
            }
        };
        if !define_status.success() {
            let _ = fs::remove_file(&disk);
            return Err(WorkloadActuatorError::Retryable(
                "virsh define rejected the Workload domain".to_string(),
            ));
        }
        Ok(())
    }

    /// Source identities are globally stable (`vm:<node>:<domain>`), while
    /// libvirt receives only the domain component. Keep that translation in the
    /// sole actuator so every caller can use one collision-resistant identity.
    fn libvirt_domain(request: &WorkloadOperationRequest) -> &str {
        request
            .workload_id
            .as_str()
            .rsplit(':')
            .next()
            .unwrap_or(request.workload_id.as_str())
    }

    fn runtime_name(request: &WorkloadOperationRequest) -> String {
        let mut name = String::from("mde-workload-");
        for value in request.workload_id.as_str().chars() {
            name.push(
                if value.is_ascii_alphanumeric() || matches!(value, '-' | '_' | '.') {
                    value
                } else {
                    '-'
                },
            );
        }
        let digest = Sha256::digest(request.workload_id.as_str().as_bytes());
        name.push('-');
        for value in &digest[..6] {
            name.push_str(&format!("{value:02x}"));
        }
        name
    }

    fn unit_name(request: &WorkloadOperationRequest) -> String {
        format!("{}.service", Self::runtime_name(request))
    }

    fn quadlet_unit_path(request: &WorkloadOperationRequest) -> PathBuf {
        Path::new(QUADLET_RUNTIME_ROOT).join(format!("{}.container", Self::runtime_name(request)))
    }

    fn render_quadlet(request: &WorkloadOperationRequest, image_ref: &str) -> String {
        format!(
            "# Managed by mackesd Workload operations; do not edit.\n[Unit]\nDescription=MCNF workload {}\n\n[Container]\nImage={}\nContainerName={}\nPodmanArgs=--root={}\n\n[Service]\nRestart=on-failure\n\n[Install]\nWantedBy=multi-user.target\n",
            request.workload_id.as_str(),
            image_ref,
            Self::runtime_name(request),
            CONTAINER_STORAGE_PATH
        )
    }

    fn ensure_container_unit(
        &self,
        request: &WorkloadOperationRequest,
    ) -> Result<(), WorkloadActuatorError> {
        let image_ref = request.image_ref.as_deref().ok_or_else(|| {
            WorkloadActuatorError::Permanent(
                "container Workload start requires an approved image_ref".to_string(),
            )
        })?;
        let artifact = self
            .approved_container_image(image_ref)
            .map_err(WorkloadActuatorError::Permanent)?;

        let mut image_exists = Command::new("podman");
        image_exists.args([
            "--root",
            CONTAINER_STORAGE_PATH,
            "image",
            "exists",
            image_ref,
        ]);
        let image_status =
            status_with_timeout(image_exists, DEFAULT_CMD_TIMEOUT).map_err(|error| {
                WorkloadActuatorError::Retryable(format!(
                    "podman image check failed to start: {error}"
                ))
            })?;
        if !image_status.success() {
            let mut load = Command::new("podman");
            load.args([
                "--root",
                CONTAINER_STORAGE_PATH,
                "load",
                "--input",
                &artifact.to_string_lossy(),
            ]);
            let load_status = status_with_timeout(load, DEFAULT_CMD_TIMEOUT).map_err(|error| {
                WorkloadActuatorError::Retryable(format!(
                    "podman image load failed to start: {error}"
                ))
            })?;
            if !load_status.success() {
                return Err(WorkloadActuatorError::Retryable(
                    "podman image load rejected the approved OCI artifact".to_string(),
                ));
            }
        }

        let root = Path::new(QUADLET_RUNTIME_ROOT);
        let root_metadata = fs::symlink_metadata(root).map_err(|error| {
            WorkloadActuatorError::Retryable(format!("inspect Quadlet runtime root: {error}"))
        })?;
        if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
            return Err(WorkloadActuatorError::Permanent(
                "Quadlet runtime root is not a regular directory".to_string(),
            ));
        }
        let path = Self::quadlet_unit_path(request);
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(WorkloadActuatorError::Permanent(
                    "managed Quadlet unit is not a regular file".to_string(),
                ));
            }
        }
        let temporary = root.join(format!(
            ".mde-workload-{}.container.{}",
            request.workload_id.as_str(),
            std::process::id()
        ));
        fs::write(&temporary, Self::render_quadlet(request, image_ref)).map_err(|error| {
            WorkloadActuatorError::Retryable(format!("write managed Quadlet unit: {error}"))
        })?;
        if let Err(error) = fs::rename(&temporary, &path) {
            let _ = fs::remove_file(&temporary);
            return Err(WorkloadActuatorError::Retryable(format!(
                "install managed Quadlet unit: {error}"
            )));
        }

        let mut reload = Command::new("systemctl");
        reload.args(["--system", "daemon-reload"]);
        let reload_status = status_with_timeout(reload, DEFAULT_CMD_TIMEOUT).map_err(|error| {
            WorkloadActuatorError::Retryable(format!(
                "systemd daemon-reload failed to start: {error}"
            ))
        })?;
        if !reload_status.success() {
            return Err(WorkloadActuatorError::Retryable(
                "systemd daemon-reload rejected the managed Quadlet unit".to_string(),
            ));
        }
        Ok(())
    }

    fn remove_container_unit(request: &WorkloadOperationRequest) -> Result<(), String> {
        let path = Self::quadlet_unit_path(request);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err("managed Quadlet unit is not a regular file".to_string());
            }
            Ok(_) => fs::remove_file(&path)
                .map_err(|error| format!("remove managed Quadlet unit: {error}"))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("inspect managed Quadlet unit: {error}")),
        }
        let mut reload = Command::new("systemctl");
        reload.args(["--system", "daemon-reload"]);
        let status = status_with_timeout(reload, DEFAULT_CMD_TIMEOUT)
            .map_err(|error| format!("systemd daemon-reload failed to start: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err("systemd daemon-reload rejected Quadlet removal".to_string())
        }
    }

    fn vm_overlay_path(request: &WorkloadOperationRequest) -> PathBuf {
        Path::new("/var/lib/mde-vms").join(format!("{}.qcow2", Self::libvirt_domain(request)))
    }

    /// Tear down one managed VM in an idempotent, ordered sequence.  The
    /// overlay is removed only after libvirt has accepted `undefine`, so a
    /// failed backend call cannot strand a running domain on a deleted disk.
    fn destroy_vm(request: &WorkloadOperationRequest) -> Result<(), String> {
        let domain = Self::libvirt_domain(request);
        let mut destroy = Command::new("virsh");
        destroy.args(["--connect", "qemu:///system", "destroy", domain]);
        let destroy_output = output_with_timeout(destroy, DEFAULT_CMD_TIMEOUT)
            .map_err(|error| format!("destroy actuator failed to start: {error}"))?;
        if !destroy_output.status.success() {
            let detail = String::from_utf8_lossy(&destroy_output.stderr);
            if !libvirt_domain_absent_or_stopped(&detail) {
                return Err(format!(
                    "destroy actuator exited with {}: {}",
                    destroy_output.status,
                    bounded_reason(detail.trim())
                ));
            }
        }

        let mut undefine = Command::new("virsh");
        undefine.args([
            "--connect",
            "qemu:///system",
            "undefine",
            domain,
            "--managed-save",
            "--nvram",
        ]);
        let undefine_output = output_with_timeout(undefine, DEFAULT_CMD_TIMEOUT)
            .map_err(|error| format!("undefine actuator failed to start: {error}"))?;
        if !undefine_output.status.success() {
            let detail = String::from_utf8_lossy(&undefine_output.stderr);
            if !libvirt_domain_absent_or_stopped(&detail) {
                return Err(format!(
                    "undefine actuator exited with {}: {}",
                    undefine_output.status,
                    bounded_reason(detail.trim())
                ));
            }
        }

        let disk = Self::vm_overlay_path(request);
        match fs::remove_file(&disk) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!(
                "remove managed VM overlay {}: {error}",
                disk.display()
            )),
        }
    }

    fn run_power_command(request: &WorkloadOperationRequest, verb: &str) -> Result<(), String> {
        let status = match request.backend {
            WorkloadBackend::LibvirtVirtqemud => {
                let mut command = Command::new("virsh");
                command.args([
                    "--connect",
                    "qemu:///system",
                    verb,
                    Self::libvirt_domain(request),
                ]);
                status_with_timeout(command, DEFAULT_CMD_TIMEOUT)
            }
            WorkloadBackend::QuadletSystemd => {
                let mut command = Command::new("systemctl");
                command.args(["--system", verb, &Self::unit_name(request)]);
                status_with_timeout(command, DEFAULT_CMD_TIMEOUT)
            }
        }
        .map_err(|error| format!("{verb} actuator failed to start: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("{verb} actuator exited with {status}"))
        }
    }

    fn running(request: &WorkloadOperationRequest) -> Result<bool, String> {
        let output = match request.backend {
            WorkloadBackend::LibvirtVirtqemud => {
                let mut command = Command::new("virsh");
                command.args([
                    "--connect",
                    "qemu:///system",
                    "domstate",
                    Self::libvirt_domain(request),
                ]);
                output_with_timeout(command, DEFAULT_CMD_TIMEOUT)
            }
            WorkloadBackend::QuadletSystemd => {
                let mut command = Command::new("systemctl");
                command.args(["--system", "is-active", &Self::unit_name(request)]);
                output_with_timeout(command, DEFAULT_CMD_TIMEOUT)
            }
        }
        .map_err(|error| format!("state observation failed: {error}"))?;
        let state = String::from_utf8_lossy(&output.stdout);
        Ok(output.status.success()
            && (state.trim().eq_ignore_ascii_case("running")
                || state.trim().eq_ignore_ascii_case("active")))
    }
}

impl SystemWorkloadActuator {
    fn checked_domain(vm_id: &str) -> Result<String, WorkloadActuatorError> {
        mackes_mesh_types::workloads::WorkloadId::new(vm_id.trim())
            .map(|id| id.into_string())
            .map_err(|error| {
                WorkloadActuatorError::Permanent(format!(
                    "invalid migration workload identity: {error}"
                ))
            })
    }

    fn virsh_output(
        vm_id: &str,
        verb: &str,
    ) -> Result<std::process::Output, WorkloadActuatorError> {
        let domain = Self::checked_domain(vm_id)?;
        let mut command = Command::new("virsh");
        command.args(["--connect", "qemu:///system", verb, &domain]);
        output_with_timeout(command, DEFAULT_CMD_TIMEOUT).map_err(|error| {
            WorkloadActuatorError::Retryable(format!(
                "migration {verb} actuator failed to start: {error}"
            ))
        })
    }

    fn require_success(
        output: std::process::Output,
        verb: &str,
    ) -> Result<std::process::Output, WorkloadActuatorError> {
        if output.status.success() {
            return Ok(output);
        }
        Err(WorkloadActuatorError::Retryable(format!(
            "migration {verb} actuator exited with {}: {}",
            output.status,
            bounded_reason(String::from_utf8_lossy(&output.stderr).trim())
        )))
    }
}

impl SystemWorkloadActuator {
    fn migration_capture_definition_impl(
        &self,
        vm_id: &str,
    ) -> Result<String, WorkloadActuatorError> {
        let output = Self::require_success(Self::virsh_output(vm_id, "dumpxml")?, "dumpxml")?;
        let xml = String::from_utf8_lossy(&output.stdout).into_owned();
        if xml.trim().is_empty() {
            return Err(WorkloadActuatorError::Permanent(
                "migration dumpxml returned an empty definition".into(),
            ));
        }
        Ok(xml)
    }

    fn migration_request_stop_impl(&self, vm_id: &str) -> Result<(), WorkloadActuatorError> {
        let output = Self::virsh_output(vm_id, "shutdown")?;
        if output.status.success()
            || libvirt_domain_absent_or_stopped(&String::from_utf8_lossy(&output.stderr))
        {
            return Ok(());
        }
        Self::require_success(output, "shutdown").map(|_| ())
    }

    fn migration_is_stopped_impl(&self, vm_id: &str) -> Result<bool, WorkloadActuatorError> {
        let output = Self::virsh_output(vm_id, "domstate")?;
        if !output.status.success()
            && libvirt_domain_absent_or_stopped(&String::from_utf8_lossy(&output.stderr))
        {
            return Ok(true);
        }
        let output = Self::require_success(output, "domstate")?;
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim()
            .eq_ignore_ascii_case("shut off"))
    }

    fn migration_define_and_start_impl(
        &self,
        vm_id: &str,
        domain_xml: &str,
    ) -> Result<(), WorkloadActuatorError> {
        let domain = Self::checked_domain(vm_id)?;
        if domain_xml.trim().is_empty() {
            return Err(WorkloadActuatorError::Permanent(
                "migration definition is empty".into(),
            ));
        }
        let existing = Self::virsh_output(&domain, "domstate")?;
        if existing.status.success()
            && !String::from_utf8_lossy(&existing.stdout)
                .trim()
                .eq_ignore_ascii_case("shut off")
        {
            return Ok(());
        }
        let xml_path = std::env::temp_dir().join(format!(
            "mde-workload-migrate-{domain}-{}-{}.xml",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        fs::write(&xml_path, domain_xml).map_err(|error| {
            WorkloadActuatorError::Retryable(format!("write migration definition: {error}"))
        })?;
        let mut define = Command::new("virsh");
        define.args([
            "--connect",
            "qemu:///system",
            "define",
            &xml_path.to_string_lossy(),
        ]);
        let define_result = output_with_timeout(define, DEFAULT_CMD_TIMEOUT).map_err(|error| {
            WorkloadActuatorError::Retryable(format!(
                "migration define actuator failed to start: {error}"
            ))
        });
        let _ = fs::remove_file(&xml_path);
        Self::require_success(define_result?, "define")?;
        let start = Self::virsh_output(&domain, "start")?;
        if start.status.success()
            || libvirt_domain_already_running(&String::from_utf8_lossy(&start.stderr))
        {
            Ok(())
        } else {
            Self::require_success(start, "start").map(|_| ())
        }
    }

    fn migration_relinquish_definition_impl(
        &self,
        vm_id: &str,
    ) -> Result<(), WorkloadActuatorError> {
        let output = Self::virsh_output(vm_id, "undefine")?;
        if output.status.success()
            || libvirt_domain_absent(&String::from_utf8_lossy(&output.stderr))
        {
            Ok(())
        } else {
            Self::require_success(output, "undefine").map(|_| ())
        }
    }
}

fn libvirt_domain_absent_or_stopped(detail: &str) -> bool {
    let normalized = detail.to_ascii_lowercase();
    libvirt_domain_absent(&normalized) || normalized.contains("domain is not running")
}

fn libvirt_domain_absent(detail: &str) -> bool {
    let normalized = detail.to_ascii_lowercase();
    normalized.contains("domain not found") || normalized.contains("failed to get domain")
}

fn libvirt_domain_already_running(detail: &str) -> bool {
    let normalized = detail.to_ascii_lowercase();
    normalized.contains("domain is already active")
        || normalized.contains("domain is already running")
        || normalized.contains("already active")
}

impl WorkloadActuator for SystemWorkloadActuator {
    fn migration_capture_definition(&self, vm_id: &str) -> Result<String, WorkloadActuatorError> {
        self.migration_capture_definition_impl(vm_id)
    }

    fn migration_request_stop(&self, vm_id: &str) -> Result<(), WorkloadActuatorError> {
        self.migration_request_stop_impl(vm_id)
    }

    fn migration_is_stopped(&self, vm_id: &str) -> Result<bool, WorkloadActuatorError> {
        self.migration_is_stopped_impl(vm_id)
    }

    fn migration_define_and_start(
        &self,
        vm_id: &str,
        domain_xml: &str,
    ) -> Result<(), WorkloadActuatorError> {
        self.migration_define_and_start_impl(vm_id, domain_xml)
    }

    fn migration_relinquish_definition(&self, vm_id: &str) -> Result<(), WorkloadActuatorError> {
        self.migration_relinquish_definition_impl(vm_id)
    }

    fn reap_expired(&self, now_ms: u64) {
        if let Ok(mut attachments) = self.attachments.lock() {
            attachments.retain(|_, runtime| runtime.server.lease().expires_at_ms > now_ms);
        }
    }

    fn revoke_attachment(&self, status: &WorkloadOperationStatus) {
        self.revoke_persisted_attachment(status);
    }

    fn recover_attachment(
        &self,
        request: &WorkloadOperationRequest,
        status: &WorkloadOperationStatus,
        now_ms: u64,
    ) -> Result<Option<WorkloadActuatorOutcome>, WorkloadActuatorError> {
        if request.action != WorkloadOperationAction::StartAndAttach
            || status.phase != WorkloadOperationPhase::Completed
        {
            return Ok(None);
        }
        let Some(lease) = status.attachment.as_ref() else {
            return Ok(None);
        };
        if lease.workload_id != status.workload_id
            || lease.generation != status.generation
            || lease.protocol != WorkloadAttachmentProtocol::QemuDisplay1Dmabuf
            || lease.validate(now_ms).is_err()
        {
            self.revoke_persisted_attachment(status);
            return Ok(Some(Self::recovered_attachment_unavailable(
                "the recovered Display1 lease was expired or did not match the exact workload generation and was revoked",
            )));
        }
        if !Self::running(request)? {
            self.revoke_persisted_attachment(status);
            let mut outcome = Self::recovered_attachment_unavailable(
                "the recovered workload is not running; its Display1 lease was revoked",
            );
            outcome.power = WorkloadPowerState::Stopped;
            return Ok(Some(outcome));
        }

        let runtime = self.attachment_for_status(request, status, now_ms)?;
        if runtime.registration_state() == DISPLAY1_REGISTRATION_NEW {
            runtime.register(Self::qemu_display1_address(request)?);
        }
        if runtime.registration_state() == DISPLAY1_REGISTRATION_FAILED {
            let reason = runtime
                .registration_error()
                .unwrap_or_else(|| "QEMU Display1 recovery registration failed".into());
            self.revoke_persisted_attachment(status);
            return Ok(Some(Self::recovered_attachment_unavailable(format!(
                "{reason}; the recovered lease was revoked"
            ))));
        }
        if runtime.registration_state() == DISPLAY1_REGISTRATION_READY && runtime.first_frame_seen()
        {
            return Ok(Some(WorkloadActuatorOutcome {
                phase: WorkloadOperationPhase::Completed,
                power: WorkloadPowerState::Running,
                readiness: WorkloadReadiness::Ready,
                retryable: false,
                reason: None,
                remediation: None,
                attachment: Some(lease.clone()),
            }));
        }
        Ok(Some(WorkloadActuatorOutcome {
            phase: WorkloadOperationPhase::Completed,
            power: WorkloadPowerState::Running,
            readiness: WorkloadReadiness::PreparingDisplay,
            retryable: true,
            reason: Some(
                "re-attaching the recovered exact-generation Display1 session and waiting for a validated first frame"
                    .into(),
            ),
            remediation: Some(
                "keep the shell attached; if recovery does not converge, retry from Workloads"
                    .into(),
            ),
            attachment: Some(lease.clone()),
        }))
    }

    fn apply(
        &self,
        request: &WorkloadOperationRequest,
    ) -> Result<WorkloadActuatorOutcome, WorkloadActuatorError> {
        // Cancellation is resolved by the reconciler before this boundary in
        // the normal path. Keep the adapter fail-closed as well: a direct or
        // replayed Cancel must never create a Display1 broker, define a VM, or
        // run a lifecycle command before its action is inspected.
        if request.action == WorkloadOperationAction::Cancel {
            return Ok(WorkloadActuatorOutcome {
                phase: WorkloadOperationPhase::Cancelled,
                power: WorkloadPowerState::Stopped,
                readiness: WorkloadReadiness::Unavailable,
                retryable: false,
                reason: Some("workload operation cancelled before adapter side effect".into()),
                remediation: None,
                attachment: None,
            });
        }
        // Bind the expiring node-local broker before any libvirt/systemd side
        // effect. The journal already crossed the Defining boundary, so a
        // broker failure remains retryable without ever claiming attachment.
        if matches!(
            request.action,
            WorkloadOperationAction::StartAndAttach | WorkloadOperationAction::Start
        ) && request.backend == WorkloadBackend::QuadletSystemd
        {
            self.ensure_container_unit(request)?;
        }
        let attachment = if request.action == WorkloadOperationAction::StartAndAttach {
            Some(
                self.ensure_attachment(
                    request,
                    request.expected_generation.saturating_add(1).max(1),
                    now_ms(),
                )?
                .server
                .lease()
                .clone(),
            )
        } else {
            None
        };
        if matches!(
            request.action,
            WorkloadOperationAction::StartAndAttach | WorkloadOperationAction::Start
        ) {
            match request.backend {
                WorkloadBackend::LibvirtVirtqemud
                    if request.action == WorkloadOperationAction::StartAndAttach =>
                {
                    self.define_vm(request)?;
                }
                WorkloadBackend::QuadletSystemd => {}
                WorkloadBackend::LibvirtVirtqemud => {}
            }
        }
        let outcome = match request.action {
            WorkloadOperationAction::Cancel => WorkloadActuatorOutcome {
                phase: WorkloadOperationPhase::Cancelled,
                power: WorkloadPowerState::Stopped,
                readiness: WorkloadReadiness::Unavailable,
                retryable: false,
                reason: Some("operator cancelled the workload operation".into()),
                remediation: None,
                attachment: None,
            },
            WorkloadOperationAction::StartAndAttach | WorkloadOperationAction::Start => {
                Self::run_power_command(request, "start")?;
                WorkloadActuatorOutcome {
                    phase: WorkloadOperationPhase::WaitingForGuest,
                    power: WorkloadPowerState::Starting,
                    readiness: WorkloadReadiness::WaitingForGuest,
                    retryable: true,
                    reason: None,
                    remediation: None,
                    attachment,
                }
            }
            WorkloadOperationAction::Restart => {
                Self::run_power_command(request, "restart")?;
                WorkloadActuatorOutcome {
                    phase: WorkloadOperationPhase::WaitingForGuest,
                    power: WorkloadPowerState::Starting,
                    readiness: WorkloadReadiness::WaitingForGuest,
                    retryable: true,
                    reason: None,
                    remediation: None,
                    attachment: None,
                }
            }
            WorkloadOperationAction::Destroy => {
                if request.backend.is_vm() {
                    Self::destroy_vm(request)?;
                } else {
                    Self::run_power_command(request, "disable")?;
                    Self::remove_container_unit(request)
                        .map_err(WorkloadActuatorError::Retryable)?;
                }
                self.remove_attachment(request);
                WorkloadActuatorOutcome {
                    phase: WorkloadOperationPhase::Completed,
                    power: WorkloadPowerState::Stopped,
                    readiness: WorkloadReadiness::Unavailable,
                    retryable: false,
                    reason: None,
                    remediation: None,
                    attachment: None,
                }
            }
            WorkloadOperationAction::Stop => {
                let verb = if request.backend.is_vm() {
                    "shutdown"
                } else {
                    "stop"
                };
                Self::run_power_command(request, verb)?;
                WorkloadActuatorOutcome {
                    phase: WorkloadOperationPhase::Stopping,
                    power: WorkloadPowerState::Stopping,
                    readiness: WorkloadReadiness::Unavailable,
                    retryable: true,
                    reason: None,
                    remediation: None,
                    attachment: None,
                }
            }
            WorkloadOperationAction::Pause => {
                Self::run_power_command(request, "suspend")?;
                WorkloadActuatorOutcome {
                    phase: WorkloadOperationPhase::Completed,
                    power: WorkloadPowerState::Paused,
                    readiness: WorkloadReadiness::Degraded,
                    retryable: false,
                    reason: None,
                    remediation: None,
                    attachment: None,
                }
            }
            WorkloadOperationAction::Resume => {
                Self::run_power_command(request, "resume")?;
                WorkloadActuatorOutcome {
                    phase: WorkloadOperationPhase::WaitingForService,
                    power: WorkloadPowerState::Running,
                    readiness: WorkloadReadiness::WaitingForService,
                    retryable: true,
                    reason: None,
                    remediation: None,
                    attachment: None,
                }
            }
            WorkloadOperationAction::Open | WorkloadOperationAction::Reconcile => {
                return self
                    .observe(request, &queued_status(request))
                    .and_then(|outcome| {
                        outcome.ok_or_else(|| "workload is not yet observable".into())
                    });
            }
        };
        Ok(outcome)
    }

    fn cancel(
        &self,
        request: &WorkloadOperationRequest,
        _status: &WorkloadOperationStatus,
    ) -> Result<WorkloadActuatorOutcome, WorkloadActuatorError> {
        // A canceled StartAndAttach owns the domain and overlay it may have
        // created, so remove both through the same libvirt authority. Other
        // lifecycle operations preserve the workload definition and only
        // stop the active unit/domain. Both paths release the node-local
        // Display1 runtime before reporting terminal cancellation.
        if request.action == WorkloadOperationAction::StartAndAttach
            && request.backend == WorkloadBackend::LibvirtVirtqemud
        {
            Self::destroy_vm(request).map_err(WorkloadActuatorError::Retryable)?;
            self.remove_attachment(request);
            return Ok(WorkloadActuatorOutcome {
                phase: WorkloadOperationPhase::Cancelled,
                power: WorkloadPowerState::Stopped,
                readiness: WorkloadReadiness::Unavailable,
                retryable: false,
                reason: Some("target operation cancelled and VM resources cleaned up".into()),
                remediation: None,
                attachment: None,
            });
        }

        if !Self::running(request).map_err(WorkloadActuatorError::Retryable)? {
            self.remove_attachment(request);
            return Ok(WorkloadActuatorOutcome {
                phase: WorkloadOperationPhase::Cancelled,
                power: WorkloadPowerState::Stopped,
                readiness: WorkloadReadiness::Unavailable,
                retryable: false,
                reason: Some("target operation was already stopped".into()),
                remediation: None,
                attachment: None,
            });
        }

        let verb = if request.backend.is_vm() {
            "shutdown"
        } else {
            "stop"
        };
        Self::run_power_command(request, verb).map_err(WorkloadActuatorError::Retryable)?;
        if Self::running(request).map_err(WorkloadActuatorError::Retryable)? {
            return Ok(WorkloadActuatorOutcome {
                phase: WorkloadOperationPhase::Stopping,
                power: WorkloadPowerState::Stopping,
                readiness: WorkloadReadiness::Unavailable,
                retryable: true,
                reason: Some("target operation is stopping".into()),
                remediation: Some("the adapter will re-check the target before cleanup".into()),
                attachment: None,
            });
        }
        self.remove_attachment(request);
        Ok(WorkloadActuatorOutcome {
            phase: WorkloadOperationPhase::Cancelled,
            power: WorkloadPowerState::Stopped,
            readiness: WorkloadReadiness::Unavailable,
            retryable: false,
            reason: Some("target operation cancelled and backend stopped".into()),
            remediation: None,
            attachment: None,
        })
    }

    fn observe(
        &self,
        request: &WorkloadOperationRequest,
        status: &WorkloadOperationStatus,
    ) -> Result<Option<WorkloadActuatorOutcome>, WorkloadActuatorError> {
        let running = Self::running(request)?;
        let outcome = if running {
            match status.phase {
                WorkloadOperationPhase::WaitingForGuest => WorkloadActuatorOutcome {
                    phase: WorkloadOperationPhase::WaitingForService,
                    power: WorkloadPowerState::Running,
                    readiness: WorkloadReadiness::WaitingForService,
                    retryable: true,
                    reason: None,
                    remediation: None,
                    attachment: None,
                },
                WorkloadOperationPhase::WaitingForService
                    if request.action == WorkloadOperationAction::StartAndAttach =>
                {
                    let runtime = self.attachment_for_status(request, status, now_ms())?;
                    let address = Self::qemu_display1_address(request)?;
                    runtime.register(address);
                    WorkloadActuatorOutcome {
                        phase: WorkloadOperationPhase::PreparingDisplay,
                        power: WorkloadPowerState::Running,
                        readiness: WorkloadReadiness::PreparingDisplay,
                        retryable: true,
                        reason: Some(
                            "Workload is running; registering the QEMU Display1 listener".into(),
                        ),
                        remediation: Some(
                            "keep the shell attached; completion requires a validated first frame"
                                .into(),
                        ),
                        attachment: Some(runtime.server.lease().clone()),
                    }
                }
                WorkloadOperationPhase::PreparingDisplay
                | WorkloadOperationPhase::WaitingForFirstFrame
                    if request.action == WorkloadOperationAction::StartAndAttach =>
                {
                    let runtime = self.attachment_for_status(request, status, now_ms())?;
                    if runtime.registration_state() == DISPLAY1_REGISTRATION_NEW {
                        let address = Self::qemu_display1_address(request)?;
                        runtime.register(address);
                    }
                    if runtime.registration_state() == DISPLAY1_REGISTRATION_FAILED {
                        let reason = runtime
                            .registration_error()
                            .unwrap_or_else(|| "QEMU Display1 listener registration failed".into());
                        return Err(WorkloadActuatorError::Retryable(reason));
                    }
                    if runtime.registration_state() == DISPLAY1_REGISTRATION_READY
                        && runtime.first_frame_seen()
                    {
                        return Ok(Some(WorkloadActuatorOutcome {
                            phase: WorkloadOperationPhase::Completed,
                            power: WorkloadPowerState::Running,
                            readiness: WorkloadReadiness::Ready,
                            retryable: false,
                            reason: None,
                            remediation: None,
                            attachment: Some(runtime.server.lease().clone()),
                        }));
                    }
                    let (phase, reason) =
                        if runtime.registration_state() == DISPLAY1_REGISTRATION_PENDING {
                            (
                                WorkloadOperationPhase::PreparingDisplay,
                                "waiting for QEMU to accept the authenticated Display1 listener",
                            )
                        } else {
                            (
                                WorkloadOperationPhase::WaitingForFirstFrame,
                                "Display1 has not delivered a validated first frame yet",
                            )
                        };
                    WorkloadActuatorOutcome {
                        phase,
                        power: WorkloadPowerState::Running,
                        readiness: WorkloadReadiness::PreparingDisplay,
                        retryable: true,
                        reason: Some(reason.into()),
                        remediation: Some(
                            "verify the node-local broker, KMS import, and shell attachment".into(),
                        ),
                        attachment: Some(runtime.server.lease().clone()),
                    }
                }
                WorkloadOperationPhase::WaitingForService
                | WorkloadOperationPhase::PreparingDisplay
                | WorkloadOperationPhase::WaitingForFirstFrame
                    if matches!(
                        request.action,
                        WorkloadOperationAction::Start
                            | WorkloadOperationAction::Restart
                            | WorkloadOperationAction::Resume
                    ) =>
                {
                    WorkloadActuatorOutcome {
                        phase: WorkloadOperationPhase::Completed,
                        power: WorkloadPowerState::Running,
                        readiness: WorkloadReadiness::Ready,
                        retryable: false,
                        reason: None,
                        remediation: None,
                        attachment: None,
                    }
                }
                WorkloadOperationPhase::Stopping => WorkloadActuatorOutcome {
                    phase: WorkloadOperationPhase::Stopping,
                    power: WorkloadPowerState::Stopping,
                    readiness: WorkloadReadiness::Unavailable,
                    retryable: true,
                    reason: None,
                    remediation: None,
                    attachment: None,
                },
                _ => return Ok(None),
            }
        } else if matches!(
            status.phase,
            WorkloadOperationPhase::Stopping | WorkloadOperationPhase::WaitingForGuest
        ) {
            self.stopped_outcome(request)
        } else {
            return Ok(None);
        };
        Ok(Some(outcome))
    }
}

fn queued_status(request: &WorkloadOperationRequest) -> WorkloadOperationStatus {
    WorkloadOperationStatus {
        schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
        request_id: request.request_id.clone(),
        workload_id: request.workload_id.clone(),
        backend: request.backend,
        resources: request.resources,
        image_ref: request.image_ref.clone(),
        generation: request.expected_generation.max(1),
        phase: WorkloadOperationPhase::Queued,
        power: WorkloadPowerState::Defined,
        readiness: WorkloadReadiness::Unknown,
        signals: WorkloadRuntimeSignals::default(),
        retryable: false,
        attempt: 0,
        next_retry_at_ms: 0,
        reason: None,
        remediation: None,
        attachment: None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HandleResult {
    Accepted(WorkloadOperationStatus),
    Rejected(WorkloadOperationErrorCode),
}

impl HandleResult {
    fn reply(self, request_id: String) -> WorkloadOperationReply {
        match self {
            Self::Accepted(status) => WorkloadOperationReply {
                schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
                request_id,
                accepted: true,
                status: Some(status),
                error_code: None,
            },
            Self::Rejected(error_code) => WorkloadOperationReply {
                schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
                request_id,
                accepted: false,
                status: None,
                error_code: Some(error_code),
            },
        }
    }
}

/// The sole compute worker for Workload operations.
pub struct WorkloadComputeWorker {
    node_id: String,
    role_rank: u8,
    bus_root: Option<PathBuf>,
    state_root: PathBuf,
    poll_interval: Duration,
    actuator: Box<dyn WorkloadActuator>,
    authorizer: Box<dyn WorkloadAuthorizer>,
    capacity_override: Option<HostCapacity>,
    storage_capacity_override: Option<WorkloadStorageCapacity>,
    cursor: Option<String>,
    last_projection: Option<Vec<WorkloadOperationStatus>>,
    migration_sender: SyncSender<WorkloadMigrationEnvelope>,
    migration_commands: Receiver<WorkloadMigrationEnvelope>,
    migration_replay_due_ms: AtomicU64,
}

impl WorkloadComputeWorker {
    /// Construct the production worker.  Lighthouses receive the worker only
    /// when explicitly targeted and are rejected by `role_rank == 0`.
    #[must_use]
    pub fn new(node_id: String, role_rank: u8) -> Self {
        let (migration_tx, migration_commands) = sync_channel(16);
        let signer: Box<dyn TokenSigner> = HmacTokenSigner::from_systemd_credential()
            .map(|signer| Box::new(signer) as Box<dyn TokenSigner>)
            .unwrap_or_else(|_| Box::new(NullSigner));
        let state_root = crate::default_db_path()
            .parent()
            .map(|parent| parent.join("workloads"))
            .unwrap_or_else(|| PathBuf::from("/var/lib/mde/workloads"));
        Self {
            node_id,
            role_rank,
            bus_root: crate::bus_publish::default_bus_root(),
            state_root,
            poll_interval: DEFAULT_POLL_INTERVAL,
            actuator: Box::new(SystemWorkloadActuator::new(
                crate::default_db_path()
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from("/var/lib/mde")),
            )),
            authorizer: Box::new(ArmedWorkloadAuthorizer {
                signer,
                auth_root: PathBuf::from(DEFAULT_AUTH_ROOT),
            }),
            capacity_override: None,
            storage_capacity_override: None,
            cursor: None,
            last_projection: None,
            migration_sender: migration_tx,
            migration_commands,
            migration_replay_due_ms: AtomicU64::new(0),
        }
    }

    fn register_migration_executor(&self) {
        if let Ok(mut slot) = migration_executor_registry().lock() {
            *slot = Some(self.migration_sender.clone());
        }
    }

    fn execute_migration_command(
        &self,
        command: &WorkloadMigrationCommand,
    ) -> Result<WorkloadMigrationReply, WorkloadActuatorError> {
        match command {
            WorkloadMigrationCommand::CaptureDefinition { vm_id } => self
                .actuator
                .migration_capture_definition(vm_id)
                .map(WorkloadMigrationReply::Definition),
            WorkloadMigrationCommand::RequestStop { vm_id } => self
                .actuator
                .migration_request_stop(vm_id)
                .map(|()| WorkloadMigrationReply::Complete),
            WorkloadMigrationCommand::ObserveStopped { vm_id } => self
                .actuator
                .migration_is_stopped(vm_id)
                .map(WorkloadMigrationReply::Stopped),
            WorkloadMigrationCommand::DefineAndStart { vm_id, domain_xml } => self
                .actuator
                .migration_define_and_start(vm_id, domain_xml)
                .map(|()| WorkloadMigrationReply::Complete),
            WorkloadMigrationCommand::RelinquishDefinition { vm_id } => self
                .actuator
                .migration_relinquish_definition(vm_id)
                .map(|()| WorkloadMigrationReply::Complete),
        }
    }

    fn replay_migration_commands(&self, journal: &WorkloadMigrationJournal) {
        let records = match journal.pending() {
            Ok(records) => records,
            Err(error) => {
                tracing::error!(%error, "migration command journal recovery refused");
                return;
            }
        };
        for mut record in records {
            if record.phase == WorkloadMigrationJournalPhase::Applied {
                if let Err(error) = journal.remove(&record.command_id) {
                    tracing::warn!(%error, command_id = %record.command_id, "applied migration journal cleanup failed");
                }
                continue;
            }
            match self.execute_migration_command(&record.command) {
                Ok(_) | Err(WorkloadActuatorError::Permanent(_)) => {
                    record.phase = WorkloadMigrationJournalPhase::Applied;
                    if let Err(error) = journal.store(&record) {
                        tracing::error!(%error, command_id = %record.command_id, "migration recovery completion could not be journaled");
                        continue;
                    }
                    if let Err(error) = journal.remove(&record.command_id) {
                        tracing::warn!(%error, command_id = %record.command_id, "migration recovery journal cleanup failed");
                    }
                }
                Err(WorkloadActuatorError::Retryable(error)) => {
                    tracing::warn!(%error, command_id = %record.command_id, "migration recovery remains retryable");
                }
            }
        }
    }

    fn drain_migration_commands(&self) {
        let journal = match WorkloadMigrationJournal::open(&self.state_root) {
            Ok(journal) => journal,
            Err(error) => {
                tracing::error!(%error, "migration command journal unavailable");
                while let Ok(envelope) = self.migration_commands.try_recv() {
                    let _ = envelope
                        .reply
                        .send(Err(WorkloadActuatorError::Retryable(format!(
                            "Workload migration journal unavailable: {error}"
                        ))));
                }
                return;
            }
        };
        // Recovery retries are durable, but deliberately paced. A broken
        // libvirt backend must not turn the one-second worker tick into a
        // command storm across every retained migration record.
        let replay_now_ms = now_ms();
        if replay_now_ms >= self.migration_replay_due_ms.load(Ordering::Acquire) {
            self.migration_replay_due_ms.store(
                replay_now_ms.saturating_add(MAX_RETRY_BACKOFF_MS),
                Ordering::Release,
            );
            self.replay_migration_commands(&journal);
        }
        loop {
            let envelope = match self.migration_commands.try_recv() {
                Ok(envelope) => envelope,
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            };
            let mut record = WorkloadMigrationJournalRecord {
                schema_version: 1,
                command_id: envelope.command_id,
                phase: WorkloadMigrationJournalPhase::Pending,
                command: envelope.command,
            };
            if let Err(error) = journal.store(&record) {
                let _ = envelope
                    .reply
                    .send(Err(WorkloadActuatorError::Retryable(format!(
                        "Workload migration command was not journaled: {error}"
                    ))));
                continue;
            }
            let mut result = self.execute_migration_command(&record.command);
            if result.is_ok() || matches!(&result, Err(WorkloadActuatorError::Permanent(_))) {
                record.phase = WorkloadMigrationJournalPhase::Applied;
                if let Err(error) = journal.store(&record) {
                    result = Err(WorkloadActuatorError::Retryable(format!(
                        "Workload migration effect completed but journal finalization failed: {error}"
                    )));
                } else if let Err(error) = journal.remove(&record.command_id) {
                    tracing::warn!(%error, command_id = %record.command_id, "applied migration journal cleanup failed");
                }
            }
            let _ = envelope.reply.send(result);
        }
    }

    /// Override the Bus root for tests or a node-local service instance.
    #[must_use]
    pub fn with_bus_root(mut self, root: Option<PathBuf>) -> Self {
        self.bus_root = root;
        self
    }

    /// Override the durable journal root.
    #[must_use]
    pub fn with_state_root(mut self, root: PathBuf) -> Self {
        self.state_root = root;
        self
    }

    /// Override the operation poll cadence.
    #[must_use]
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Inject a fake or a future libvirt/Quadlet adapter.
    #[must_use]
    pub fn with_actuator(mut self, actuator: Box<dyn WorkloadActuator>) -> Self {
        self.actuator = actuator;
        self
    }

    /// Inject an authorization verifier in tests or the local-seat shell path.
    #[must_use]
    pub fn with_authorizer(mut self, authorizer: Box<dyn WorkloadAuthorizer>) -> Self {
        self.authorizer = authorizer;
        self
    }

    /// Inject a deterministic capacity sample for hostile/unit tests. The
    /// production worker always probes the managed pool and fails closed when
    /// it cannot obtain a sample.
    #[must_use]
    pub fn with_capacity(mut self, capacity: HostCapacity) -> Self {
        self.capacity_override = Some(capacity);
        self.storage_capacity_override = Some(WorkloadStorageCapacity {
            vm_storage_gb: capacity.storage_gb,
            allocated_vm_storage_gb: capacity.allocated_storage_gb,
            container_storage_gb: capacity.storage_gb,
            allocated_container_storage_gb: capacity.allocated_storage_gb,
        });
        self
    }

    /// Inject independent VM and container storage samples for hostile tests.
    #[must_use]
    pub fn with_storage_capacity(mut self, capacity: WorkloadStorageCapacity) -> Self {
        self.storage_capacity_override = Some(capacity);
        self
    }

    fn capacities<'a>(
        &self,
        statuses: impl Iterator<Item = &'a WorkloadOperationStatus>,
    ) -> (HostCapacity, WorkloadStorageCapacity) {
        if let (Some(host), Some(storage)) =
            (self.capacity_override, self.storage_capacity_override)
        {
            return (host, storage);
        }
        live_capacity(statuses)
    }

    fn handle_request(
        &mut self,
        ledger: &mut WorkloadOperationLedger,
        raw_body: &str,
        request: WorkloadOperationRequest,
        now_ms: u64,
    ) -> HandleResult {
        let now_i64 = i64::try_from(now_ms).unwrap_or(i64::MAX);
        // An identical request-id is a read-only replay. Check the durable
        // record before consuming the one-use capability so a duplicate Bus
        // delivery cannot turn a valid idempotent operation into an auth error.
        if let Some(existing) = ledger.request(&request.request_id) {
            if existing == &request {
                return ledger.status(&request.request_id).cloned().map_or(
                    HandleResult::Rejected(WorkloadOperationErrorCode::JournalUnavailable),
                    HandleResult::Accepted,
                );
            }
            tracing::warn!(node = %self.node_id, request_id = %request.request_id, "conflicting workload replay refused");
            return HandleResult::Rejected(WorkloadOperationErrorCode::Conflict);
        }
        if let Err(error) = self.authorizer.authorize(raw_body, &request, now_i64) {
            tracing::warn!(node = %self.node_id, workload = %request.workload_id.as_str(), %error, "workload operation refused before journal");
            return HandleResult::Rejected(WorkloadOperationErrorCode::Unauthorized);
        }
        let status = match ledger.accept(request.clone(), now_ms) {
            Ok(status) => status,
            Err(error) => {
                tracing::warn!(node = %self.node_id, error = %error, "workload operation admission failed");
                let code = match error {
                    crate::workload_reconciler::WorkloadLedgerError::Conflict(_) => {
                        WorkloadOperationErrorCode::Conflict
                    }
                    crate::workload_reconciler::WorkloadLedgerError::StaleGeneration(_)
                    | crate::workload_reconciler::WorkloadLedgerError::Busy(_) => {
                        WorkloadOperationErrorCode::StaleGeneration
                    }
                    crate::workload_reconciler::WorkloadLedgerError::Capacity => {
                        WorkloadOperationErrorCode::JournalUnavailable
                    }
                    crate::workload_reconciler::WorkloadLedgerError::Io(_)
                    | crate::workload_reconciler::WorkloadLedgerError::Malformed
                    | crate::workload_reconciler::WorkloadLedgerError::Contract(_)
                    | crate::workload_reconciler::WorkloadLedgerError::UnknownRequest(_)
                    | crate::workload_reconciler::WorkloadLedgerError::InvalidTransition => {
                        WorkloadOperationErrorCode::JournalUnavailable
                    }
                };
                return HandleResult::Rejected(code);
            }
        };
        let request_id = request.request_id.clone();
        self.drive_accepted(ledger, request, status, now_ms);
        ledger.status(&request_id).cloned().map_or(
            HandleResult::Rejected(WorkloadOperationErrorCode::JournalUnavailable),
            HandleResult::Accepted,
        )
    }

    /// Drive a durable operation without re-authorizing it.  This is shared by
    /// the initial Bus delivery and queued/admitting recovery after restart.
    fn drive_accepted(
        &mut self,
        ledger: &mut WorkloadOperationLedger,
        request: WorkloadOperationRequest,
        mut status: WorkloadOperationStatus,
        now_ms: u64,
    ) {
        if status.phase.is_terminal() || now_ms < status.next_retry_at_ms {
            return;
        }
        if request.deadline_at_ms <= now_ms {
            self.fail(
                ledger,
                &request,
                status,
                "workload operation deadline expired before reconciliation",
                "issue a new operation from the current Workload projection",
                false,
                now_ms,
            );
            return;
        }
        if !workload_placement_allowed(self.role_rank) {
            self.fail(
                ledger,
                &request,
                status,
                "lighthouse and unrecognized roles do not host Workloads",
                "place the workload on a pinned Workstation node",
                false,
                now_ms,
            );
            return;
        }
        // Cancellation is a journal operation, not a backend lifecycle verb.
        // Resolve it after deadline and placement policy, then use the
        // explicit target operation to either cancel before side effects or
        // invoke the adapter cleanup path for an operation already past the
        // defining boundary.
        if request.action == WorkloadOperationAction::Cancel {
            self.drive_cancel(ledger, request, status, now_ms);
            return;
        }
        if status.phase == WorkloadOperationPhase::Queued {
            status = match self.advance_phase(
                ledger,
                status,
                WorkloadOperationPhase::Validating,
                now_ms,
            ) {
                Some(status) => status,
                None => return,
            };
            let (host, storage) = self.capacities(ledger.statuses());
            let admission =
                admit_workload_for_backend(request.resources, request.backend, host, storage);
            if !admission.admitted {
                let (reason, remediation) = admission_message(admission);
                self.fail(ledger, &request, status, reason, remediation, false, now_ms);
                return;
            }
            status =
                match self.advance_phase(ledger, status, WorkloadOperationPhase::Admitting, now_ms)
                {
                    Some(status) => status,
                    None => return,
                };
        }
        if status.phase == WorkloadOperationPhase::Admitting {
            // Persist the defining boundary before invoking libvirt/systemd.
            // The adapter may combine definition and start for an already
            // defined workload, but no backend effect is permitted while the
            // journal still says only Admitting.
            status = match self.advance_phase(
                ledger,
                status,
                WorkloadOperationPhase::Defining,
                now_ms,
            ) {
                Some(status) => status,
                None => return,
            };
        }
        if status.phase == WorkloadOperationPhase::Defining {
            match self.actuator.apply(&request) {
                Ok(outcome) => self.apply_outcome(ledger, &request, status, outcome, now_ms),
                Err(error) => self.retry_or_fail(ledger, &request, status, &error, now_ms),
            }
        }
    }

    fn drive_cancel(
        &mut self,
        ledger: &mut WorkloadOperationLedger,
        request: WorkloadOperationRequest,
        status: WorkloadOperationStatus,
        now_ms: u64,
    ) {
        let Some(target_id) = request.target_request_id.as_deref() else {
            self.fail(
                ledger,
                &request,
                status,
                "cancellation target is missing",
                "issue cancellation with the exact in-flight operation id",
                false,
                now_ms,
            );
            return;
        };
        let Some(target_request) = ledger.request(target_id).cloned() else {
            self.fail(
                ledger,
                &request,
                status,
                "cancellation target is no longer journaled",
                "refresh the Workload projection and issue a new operation",
                false,
                now_ms,
            );
            return;
        };
        let Some(target_status) = ledger.status(target_id).cloned() else {
            self.fail(
                ledger,
                &request,
                status,
                "cancellation target has no durable status",
                "refresh the Workload projection and issue a new operation",
                false,
                now_ms,
            );
            return;
        };
        if target_request.action == WorkloadOperationAction::Cancel
            || target_request.workload_id != request.workload_id
            || target_request.target_node != request.target_node
            || target_request.backend != request.backend
            || target_status.resources != request.resources
            || target_status.generation != request.expected_generation
        {
            self.fail(
                ledger,
                &request,
                status,
                "cancellation target does not match the requested Workload generation",
                "refresh the Workload projection and retry with its exact operation id",
                false,
                now_ms,
            );
            return;
        }

        if target_status.phase.is_terminal() {
            self.complete_cancel(
                ledger,
                status,
                "cancellation target was already terminal",
                now_ms,
            );
            return;
        }

        // No adapter effect can have occurred before Defining. Cancel the
        // target directly in the journal and keep the cancellation request's
        // own status as the newest generation for the projection.
        if matches!(
            target_status.phase,
            WorkloadOperationPhase::Queued
                | WorkloadOperationPhase::Validating
                | WorkloadOperationPhase::Admitting
        ) {
            let mut cancelled = target_status;
            cancelled.phase = WorkloadOperationPhase::Cancelled;
            cancelled.power = WorkloadPowerState::Stopped;
            cancelled.readiness = WorkloadReadiness::Unavailable;
            cancelled.signals = WorkloadRuntimeSignals::from_readiness(
                WorkloadOperationPhase::Cancelled,
                WorkloadReadiness::Unavailable,
            );
            cancelled.retryable = false;
            cancelled.next_retry_at_ms = 0;
            cancelled.reason = Some("target operation cancelled before adapter side effect".into());
            cancelled.remediation = None;
            if let Err(error) = ledger.advance(target_id, cancelled, now_ms) {
                tracing::error!(%error, "target cancellation could not be journaled");
                self.fail(
                    ledger,
                    &request,
                    status,
                    "target cancellation could not be journaled",
                    "retry after the durable Workload journal is healthy",
                    false,
                    now_ms,
                );
                return;
            }
            self.complete_cancel(
                ledger,
                status,
                "target operation cancelled before adapter side effect",
                now_ms,
            );
            return;
        }

        match self.actuator.cancel(&target_request, &target_status) {
            Ok(outcome) => {
                self.apply_outcome(ledger, &target_request, target_status, outcome, now_ms);
                let Some(updated_target) = ledger.status(target_id).cloned() else {
                    self.fail(
                        ledger,
                        &request,
                        status,
                        "target cancellation status disappeared",
                        "retry after the durable Workload journal is healthy",
                        false,
                        now_ms,
                    );
                    return;
                };
                if updated_target.phase.is_terminal() {
                    self.complete_cancel(
                        ledger,
                        status,
                        "target operation cleanup completed",
                        now_ms,
                    );
                } else {
                    self.wait_for_cancel_cleanup(ledger, &request, status, now_ms);
                }
            }
            Err(error) => self.retry_or_fail(ledger, &request, status, &error, now_ms),
        }
    }

    fn complete_cancel(
        &self,
        ledger: &mut WorkloadOperationLedger,
        mut status: WorkloadOperationStatus,
        reason: &str,
        now_ms: u64,
    ) {
        if status.phase != WorkloadOperationPhase::Stopping {
            status = match self.advance_phase(
                ledger,
                status,
                WorkloadOperationPhase::Stopping,
                now_ms,
            ) {
                Some(status) => status,
                None => return,
            };
        }
        status.phase = WorkloadOperationPhase::Completed;
        status.power = WorkloadPowerState::Stopped;
        status.readiness = WorkloadReadiness::Unavailable;
        status.signals = WorkloadRuntimeSignals::from_readiness(
            WorkloadOperationPhase::Completed,
            WorkloadReadiness::Unavailable,
        );
        status.retryable = false;
        status.next_retry_at_ms = 0;
        status.reason = Some(bounded_reason(reason));
        status.remediation = None;
        let request_id = status.request_id.clone();
        if let Err(error) = ledger.advance(&request_id, status, now_ms) {
            tracing::error!(%error, "cancellation completion could not be journaled");
        }
    }

    fn wait_for_cancel_cleanup(
        &self,
        ledger: &mut WorkloadOperationLedger,
        request: &WorkloadOperationRequest,
        mut status: WorkloadOperationStatus,
        now_ms: u64,
    ) {
        let attempt = status.attempt.saturating_add(1);
        let retryable = attempt <= MAX_ADAPTER_ATTEMPTS;
        if !retryable {
            self.fail(
                ledger,
                request,
                status,
                "target cleanup exceeded the bounded retry budget",
                "inspect the target adapter and retry from a fresh Workload operation",
                false,
                now_ms,
            );
            return;
        }
        let exponent = u32::from(attempt.saturating_sub(1).min(5));
        let delay = (1_000_u64.saturating_mul(1_u64 << exponent)).min(MAX_RETRY_BACKOFF_MS);
        status.phase = WorkloadOperationPhase::Stopping;
        status.power = WorkloadPowerState::Stopping;
        status.readiness = WorkloadReadiness::Unavailable;
        status.signals = WorkloadRuntimeSignals::from_readiness(
            WorkloadOperationPhase::Stopping,
            WorkloadReadiness::Unavailable,
        );
        status.attempt = attempt;
        status.next_retry_at_ms = now_ms.saturating_add(delay);
        status.retryable = true;
        status.reason = Some("waiting for target adapter cleanup".into());
        status.remediation = Some("the reconciler will re-check the target".into());
        let request_id = status.request_id.clone();
        if let Err(error) = ledger.advance(&request_id, status, now_ms) {
            tracing::error!(%error, "cancellation retry could not be journaled");
        }
    }

    fn advance_phase(
        &self,
        ledger: &mut WorkloadOperationLedger,
        mut status: WorkloadOperationStatus,
        phase: WorkloadOperationPhase,
        now_ms: u64,
    ) -> Option<WorkloadOperationStatus> {
        status.phase = phase;
        let request_id = status.request_id.clone();
        match ledger.advance(&request_id, status, now_ms) {
            Ok(status) => Some(status),
            Err(error) => {
                tracing::error!(%error, "workload phase could not be journaled");
                None
            }
        }
    }

    fn fail(
        &self,
        ledger: &mut WorkloadOperationLedger,
        request: &WorkloadOperationRequest,
        mut status: WorkloadOperationStatus,
        reason: &str,
        remediation: &str,
        retryable: bool,
        now_ms: u64,
    ) {
        status.phase = WorkloadOperationPhase::Failed;
        status.power = WorkloadPowerState::Failed;
        status.readiness = WorkloadReadiness::Failed;
        status.signals = WorkloadRuntimeSignals::from_readiness(
            WorkloadOperationPhase::Failed,
            WorkloadReadiness::Failed,
        );
        status.retryable = retryable;
        status.next_retry_at_ms = 0;
        status.reason = Some(bounded_reason(reason));
        status.remediation = Some(bounded_reason(remediation));
        if let Err(error) = ledger.advance(&request.request_id, status, now_ms) {
            tracing::error!(%error, "workload failure could not be journaled");
        }
    }

    fn retry_or_fail(
        &self,
        ledger: &mut WorkloadOperationLedger,
        request: &WorkloadOperationRequest,
        mut status: WorkloadOperationStatus,
        error: &WorkloadActuatorError,
        now_ms: u64,
    ) {
        let error_text = error.to_string();
        if let WorkloadActuatorError::Permanent(_) = error {
            self.fail(
                ledger,
                request,
                status,
                &error_text,
                "correct the Workload request or promote the exact approved artifact, then retry",
                false,
                now_ms,
            );
            return;
        }
        let attempt = status.attempt.saturating_add(1);
        let retryable = attempt <= MAX_ADAPTER_ATTEMPTS
            && request.deadline_at_ms > now_ms.saturating_add(1_000);
        if !retryable {
            self.fail(
                ledger,
                request,
                status,
                &bounded_reason(&error_text),
                "inspect the exact adapter reason, then retry the operation",
                false,
                now_ms,
            );
            return;
        }
        let exponent = u32::from(attempt.saturating_sub(1).min(5));
        let delay = (1_000_u64.saturating_mul(1_u64 << exponent)).min(MAX_RETRY_BACKOFF_MS);
        status.attempt = attempt;
        status.next_retry_at_ms = now_ms.saturating_add(delay);
        status.retryable = true;
        status.reason = Some(bounded_reason(&error_text));
        status.remediation = Some("adapter will retry with bounded backoff".into());
        status.signals = WorkloadRuntimeSignals::from_readiness(status.phase, status.readiness);
        if let Err(journal_error) = ledger.advance(&request.request_id, status, now_ms) {
            tracing::error!(%journal_error, "workload retry could not be journaled");
        }
    }

    fn reconcile_inflight(&mut self, ledger: &mut WorkloadOperationLedger, now_ms: u64) {
        let pending: Vec<_> = ledger
            .statuses()
            .filter(|status| {
                matches!(
                    status.phase,
                    WorkloadOperationPhase::Admitting
                        | WorkloadOperationPhase::Queued
                        | WorkloadOperationPhase::Defining
                        | WorkloadOperationPhase::Starting
                        | WorkloadOperationPhase::WaitingForGuest
                        | WorkloadOperationPhase::WaitingForService
                        | WorkloadOperationPhase::PreparingDisplay
                        | WorkloadOperationPhase::WaitingForFirstFrame
                        | WorkloadOperationPhase::Stopping
                )
            })
            .cloned()
            .collect();
        for status in pending {
            let Some(request) = ledger.request(&status.request_id).cloned() else {
                continue;
            };
            if request.deadline_at_ms <= now_ms {
                self.fail(
                    ledger,
                    &request,
                    status,
                    "workload operation deadline expired while waiting for readiness",
                    "issue a new operation from the current Workload projection",
                    false,
                    now_ms,
                );
                continue;
            }
            // Backoff is durable state, not merely an admission-time hint.
            // Honor it for post-admission observations as well as for the
            // queued/defining path, otherwise every poll tick can turn one
            // transient adapter error into an unbounded restart storm.
            if now_ms < status.next_retry_at_ms {
                continue;
            }
            if request.action == WorkloadOperationAction::Cancel {
                self.drive_accepted(ledger, request, status, now_ms);
                continue;
            }
            if matches!(
                status.phase,
                WorkloadOperationPhase::Queued
                    | WorkloadOperationPhase::Admitting
                    | WorkloadOperationPhase::Defining
            ) {
                self.drive_accepted(ledger, request, status, now_ms);
                continue;
            }
            match self.actuator.observe(&request, &status) {
                Ok(Some(outcome)) => self.apply_outcome(ledger, &request, status, outcome, now_ms),
                Ok(None) => {}
                Err(error) => self.retry_or_fail(ledger, &request, status, &error, now_ms),
            }
        }
    }

    fn refuse_recovered_attachment(
        &self,
        ledger: &mut WorkloadOperationLedger,
        mut status: WorkloadOperationStatus,
        reason: impl Into<String>,
        now_ms: u64,
    ) {
        self.actuator.revoke_attachment(&status);
        let request_id = status.request_id.clone();
        status.readiness = WorkloadReadiness::Unavailable;
        status.retryable = false;
        status.next_retry_at_ms = 0;
        status.reason = Some(reason.into());
        status.remediation = Some(
            "return to Workloads and issue a new Start and attach operation from the current generation"
                .into(),
        );
        status.attachment = None;
        status.signals = WorkloadRuntimeSignals::from_readiness(status.phase, status.readiness);
        if let Err(error) = ledger.advance(&request_id, status, now_ms) {
            tracing::error!(%error, %request_id, "recovered attachment refusal could not be journaled");
        }
    }

    fn reconcile_recovered_attachments(
        &mut self,
        ledger: &mut WorkloadOperationLedger,
        now_ms: u64,
    ) {
        let latest_generation = ledger
            .statuses()
            .fold(BTreeMap::new(), |mut latest, status| {
                latest
                    .entry(status.workload_id.as_str().to_owned())
                    .and_modify(|generation: &mut u64| {
                        *generation = (*generation).max(status.generation)
                    })
                    .or_insert(status.generation);
                latest
            });
        let recovered: Vec<_> = ledger
            .statuses()
            .filter(|status| status.phase.is_terminal() && status.attachment.is_some())
            .cloned()
            .collect();

        for mut status in recovered {
            if latest_generation.get(status.workload_id.as_str()) != Some(&status.generation) {
                self.refuse_recovered_attachment(
                    ledger,
                    status,
                    "the recovered Display1 lease belongs to a superseded workload generation and was revoked",
                    now_ms,
                );
                continue;
            }
            let Some(request) = ledger.request(&status.request_id).cloned() else {
                self.refuse_recovered_attachment(
                    ledger,
                    status,
                    "the recovered Display1 lease has no durable owning Workload request and was revoked",
                    now_ms,
                );
                continue;
            };
            if status.phase != WorkloadOperationPhase::Completed
                || request.action != WorkloadOperationAction::StartAndAttach
            {
                self.refuse_recovered_attachment(
                    ledger,
                    status,
                    "the recovered Display1 lease is not owned by a completed Start and attach operation and was revoked",
                    now_ms,
                );
                continue;
            }
            let outcome = match self.actuator.recover_attachment(&request, &status, now_ms) {
                Ok(Some(outcome)) => outcome,
                Ok(None) => continue,
                Err(error) => {
                    self.refuse_recovered_attachment(
                        ledger,
                        status,
                        bounded_reason(&format!(
                            "recovered Display1 attachment was refused: {error}"
                        )),
                        now_ms,
                    );
                    continue;
                }
            };
            if outcome.phase != WorkloadOperationPhase::Completed {
                tracing::error!(
                    request_id = %request.request_id,
                    phase = ?outcome.phase,
                    "terminal attachment recovery returned a nonterminal phase"
                );
                self.refuse_recovered_attachment(
                    ledger,
                    status,
                    "recovered Display1 attachment returned an invalid lifecycle phase and was revoked",
                    now_ms,
                );
                continue;
            }
            if let Some(lease) = outcome.attachment.as_ref() {
                if lease.workload_id != status.workload_id
                    || lease.generation != status.generation
                    || lease.protocol != WorkloadAttachmentProtocol::QemuDisplay1Dmabuf
                    || lease.validate(now_ms).is_err()
                {
                    tracing::error!(
                        request_id = %request.request_id,
                        "terminal attachment recovery returned mismatched lease identity"
                    );
                    self.refuse_recovered_attachment(
                        ledger,
                        status,
                        "recovered Display1 attachment returned mismatched identity and was refused"
                            .to_owned(),
                        now_ms,
                    );
                    continue;
                } else {
                    status.power = outcome.power;
                    status.readiness = outcome.readiness;
                    status.retryable = outcome.retryable;
                    status.reason = outcome.reason;
                    status.remediation = outcome.remediation;
                    status.attachment = Some(lease.clone());
                }
            } else {
                status.power = outcome.power;
                status.readiness = outcome.readiness;
                status.retryable = outcome.retryable;
                status.reason = outcome.reason;
                status.remediation = outcome.remediation;
                status.attachment = None;
            }
            status.next_retry_at_ms = 0;
            status.signals = WorkloadRuntimeSignals::from_readiness(status.phase, status.readiness);
            if ledger.status(&request.request_id) == Some(&status) {
                continue;
            }
            if let Err(error) = ledger.advance(&request.request_id, status, now_ms) {
                tracing::error!(%error, "recovered attachment status could not be journaled");
            }
        }
    }

    fn apply_outcome(
        &self,
        ledger: &mut WorkloadOperationLedger,
        request: &WorkloadOperationRequest,
        mut status: WorkloadOperationStatus,
        outcome: WorkloadActuatorOutcome,
        now_ms: u64,
    ) {
        // A running VM is not an attached VM.  Never turn an adapter's
        // optimistic/legacy completion into a successful StartAndAttach
        // status unless it supplied both a real lease and a Ready result
        // after the first frame was observed.
        if request.action == WorkloadOperationAction::StartAndAttach
            && outcome.phase == WorkloadOperationPhase::Completed
            && (outcome.readiness != WorkloadReadiness::Ready || outcome.attachment.is_none())
        {
            self.fail(
                ledger,
                request,
                status,
                "StartAndAttach completed without an authenticated first-frame lease",
                "repair the Display1 broker and KMS attachment, then retry the operation",
                false,
                now_ms,
            );
            return;
        }
        let steps = match phase_steps(status.phase, outcome.phase) {
            Some(steps) => steps,
            None => {
                tracing::error!(from = ?status.phase, to = ?outcome.phase, "workload phase path is not legal");
                return;
            }
        };
        let request_id = request.request_id.clone();
        if outcome.attachment.is_some() {
            status.attachment = outcome.attachment.clone();
        } else if outcome.phase == WorkloadOperationPhase::Cancelled {
            status.attachment = None;
        }
        for phase in steps {
            status.phase = phase;
            status.signals = WorkloadRuntimeSignals::from_readiness(phase, status.readiness);
            if phase == outcome.phase {
                status.power = outcome.power;
                status.readiness = outcome.readiness;
                status.signals =
                    WorkloadRuntimeSignals::from_readiness(outcome.phase, outcome.readiness);
                status.retryable = outcome.retryable;
                status.next_retry_at_ms = 0;
                status.reason = outcome.reason.clone();
                status.remediation = outcome.remediation.clone();
            }
            match ledger.advance(&request_id, status.clone(), now_ms) {
                Ok(next) => status = next,
                Err(error) => {
                    tracing::error!(%error, "workload observation result could not be journaled");
                    return;
                }
            }
        }
    }

    fn publish(
        &mut self,
        persist: Option<&mut Persist>,
        ledger: &WorkloadOperationLedger,
        now_ms: u64,
    ) {
        let mut latest = BTreeMap::<String, WorkloadOperationStatus>::new();
        for status in ledger.statuses() {
            let key = status.workload_id.as_str().to_string();
            if latest
                .get(&key)
                .is_none_or(|current| current.generation <= status.generation)
            {
                latest.insert(key, status.clone());
            }
        }
        let mut statuses: Vec<_> = latest.into_values().collect();
        // Leases are intentionally ephemeral. Expiration removes only the
        // descriptor from the projection; it never mutates the durable
        // operation or pretends the workload stopped.
        for status in &mut statuses {
            if status
                .attachment
                .as_ref()
                .is_some_and(|lease| lease.expires_at_ms <= now_ms)
            {
                status.attachment = None;
            }
        }
        if self.last_projection.as_ref() == Some(&statuses) {
            return;
        }
        let snapshot = WorkloadStateSnapshot {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            node: self.node_id.clone(),
            observed_at_ms: now_ms,
            workloads: statuses.clone(),
        };
        if let Err(error) = snapshot.validate(now_ms) {
            tracing::error!(%error, "workload state projection refused");
            return;
        }
        if let Some(persist) = persist {
            crate::bus_publish::publish_json(
                persist,
                &workload_state_topic(&self.node_id),
                &snapshot,
            );
        }
        self.last_projection = Some(statuses);
    }

    fn write_operation_reply(
        &self,
        persist: &Persist,
        message_ulid: &str,
        reply: &WorkloadOperationReply,
    ) {
        let body = serde_json::to_string(reply).unwrap_or_else(|_| {
            r#"{"schema_version":1,"request_id":"invalid-request","accepted":false,"status":null,"error_code":"journal_unavailable"}"#
                .to_string()
        });
        if let Err(error) = persist.write(
            &reply_topic(message_ulid),
            Priority::Default,
            None,
            Some(&body),
        ) {
            tracing::warn!(
                target: "mackesd::workload_compute",
                message_ulid,
                %error,
                "workload operation reply write failed"
            );
        }
    }

    fn tick_once(
        &mut self,
        ledger: &mut WorkloadOperationLedger,
        mut persist: Option<&mut Persist>,
    ) {
        self.drain_migration_commands();
        let now = now_ms();
        if let Some(persist_ref) = persist.as_deref_mut() {
            let messages = persist_ref.list_since_limit(
                ACTION_TOPIC,
                self.cursor.as_deref(),
                MAX_OPERATION_MESSAGES_PER_TICK,
            );
            if let Ok(messages) = messages {
                for message in messages {
                    self.cursor = Some(message.ulid.clone());
                    let body = message.body.as_deref().unwrap_or("");
                    if body.len() > MAX_WORKLOAD_WIRE_BYTES {
                        tracing::warn!("oversized workload operation refused");
                        self.write_operation_reply(
                            persist_ref,
                            &message.ulid,
                            &HandleResult::Rejected(WorkloadOperationErrorCode::PayloadTooLarge)
                                .reply(safe_request_id(body)),
                        );
                        continue;
                    }
                    let request = match WorkloadOperationRequest::from_json(body, now) {
                        Ok(request) => request,
                        Err(error) => {
                            tracing::warn!(%error, "malformed workload operation refused");
                            let code = match error {
                                mackes_mesh_types::workloads::WorkloadContractError::PayloadTooLarge => {
                                    WorkloadOperationErrorCode::PayloadTooLarge
                                }
                                _ => WorkloadOperationErrorCode::MalformedRequest,
                            };
                            self.write_operation_reply(
                                persist_ref,
                                &message.ulid,
                                &HandleResult::Rejected(code).reply(safe_request_id(body)),
                            );
                            continue;
                        }
                    };
                    if request.target_node != self.node_id {
                        continue;
                    }
                    let request_id = request.request_id.clone();
                    let result = self.handle_request(ledger, body, request, now);
                    self.write_operation_reply(
                        persist_ref,
                        &message.ulid,
                        &result.reply(request_id),
                    );
                }
            }
        }
        self.reconcile_inflight(ledger, now);
        self.reconcile_recovered_attachments(ledger, now);
        self.actuator.reap_expired(now);
        self.publish(persist, ledger, now);
    }
}

#[async_trait::async_trait]
impl Worker for WorkloadComputeWorker {
    fn name(&self) -> &'static str {
        "workload_compute"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        self.register_migration_executor();
        let mut ledger = WorkloadOperationLedger::open(&self.state_root)
            .map_err(|error| anyhow::anyhow!("open workload operation journal: {error}"))?;
        let mut persist = self
            .bus_root
            .clone()
            .and_then(|root| Persist::open(root).ok());
        self.tick_once(&mut ledger, persist.as_mut());
        loop {
            tokio::select! {
                () = shutdown.wait() => break,
                () = tokio::time::sleep(self.poll_interval) => {
                    self.tick_once(&mut ledger, persist.as_mut());
                }
            }
        }
        Ok(())
    }
}

fn live_capacity<'a>(
    statuses: impl Iterator<Item = &'a WorkloadOperationStatus>,
) -> (HostCapacity, WorkloadStorageCapacity) {
    let logical_cpus = std::thread::available_parallelism()
        .ok()
        .and_then(|value| u16::try_from(value.get()).ok())
        // An unavailable probe is not a one-thread host. Treat it as unknown
        // so admission rejects the request instead of allocating against a
        // made-up capacity sample.
        .unwrap_or(0);
    let memory_mb = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|body| {
            body.lines()
                .find(|line| line.starts_with("MemTotal:"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|value| value.parse::<u32>().ok())
                .map(|kb| kb / 1024)
        })
        .unwrap_or(0);
    let mut latest = BTreeMap::<&str, &WorkloadOperationStatus>::new();
    for status in statuses {
        let key = status.workload_id.as_str();
        if latest
            .get(key)
            .is_none_or(|current| current.generation <= status.generation)
        {
            latest.insert(key, status);
        }
    }
    let mut allocated_vcpu = 0_u16;
    let mut allocated_memory_mb = 0_u32;
    let mut allocated_vm_storage_gb = 0_u32;
    let mut allocated_container_storage_gb = 0_u32;
    for status in latest
        .into_values()
        .filter(|status| !status.phase.is_terminal())
    {
        allocated_vcpu = allocated_vcpu.saturating_add(status.resources.vcpu);
        allocated_memory_mb = allocated_memory_mb.saturating_add(status.resources.memory_mb);
        match status.backend {
            WorkloadBackend::LibvirtVirtqemud => {
                allocated_vm_storage_gb =
                    allocated_vm_storage_gb.saturating_add(status.resources.disk_gb);
            }
            WorkloadBackend::QuadletSystemd => {
                allocated_container_storage_gb =
                    allocated_container_storage_gb.saturating_add(status.resources.disk_gb);
            }
        }
    }
    let vm_storage_gb = probe_storage_gb(VM_STORAGE_PATH);
    let container_storage_gb = probe_storage_gb(CONTAINER_STORAGE_PATH);
    let host = HostCapacity {
        logical_cpus,
        memory_mb,
        allocated_vcpu,
        allocated_memory_mb,
        // Preserve the legacy HostCapacity shape for VM-only callers while
        // keeping the typed container pool separate below.
        storage_gb: vm_storage_gb.saturating_add(allocated_vm_storage_gb),
        allocated_storage_gb: allocated_vm_storage_gb,
    };
    let storage = WorkloadStorageCapacity {
        vm_storage_gb,
        allocated_vm_storage_gb,
        container_storage_gb,
        allocated_container_storage_gb,
    };
    (host, storage)
}

fn probe_storage_gb(path: &str) -> u32 {
    std::process::Command::new("df")
        .args(["-Pk", path])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|body| {
            body.lines()
                .nth(1)
                .and_then(|line| line.split_whitespace().nth(3).map(str::to_owned))
        })
        .and_then(|value| value.parse::<u64>().ok())
        .map(|kib| u32::try_from(kib / 1_048_576).unwrap_or(u32::MAX))
        .unwrap_or(0)
}

fn admission_message(admission: WorkloadAdmission) -> (&'static str, &'static str) {
    match admission.denial {
        Some(mackes_mesh_types::workloads::AdmissionDenial::InvalidHost) => (
            "host capacity is unknown or invalid",
            "refresh host capacity and retry on a Workstation with a valid reserve",
        ),
        Some(mackes_mesh_types::workloads::AdmissionDenial::InvalidRequest) => (
            "workload resources are outside the bounded contract",
            "choose the saved Standard or Interactive profile",
        ),
        Some(mackes_mesh_types::workloads::AdmissionDenial::CpuReserve) => (
            "workload would consume the reserved host CPU",
            "stop another workload or choose the Standard profile",
        ),
        Some(mackes_mesh_types::workloads::AdmissionDenial::MemoryReserve) => (
            "workload would consume the reserved host memory",
            "stop another workload or choose the Standard profile",
        ),
        Some(mackes_mesh_types::workloads::AdmissionDenial::StorageReserve) => (
            "the managed storage pool for this Workload backend has insufficient free space",
            "create or select the previewed workload pool, then retry",
        ),
        None => (
            "workload admission failed",
            "retry after refreshing capacity",
        ),
    }
}

fn phase_steps(
    from: WorkloadOperationPhase,
    to: WorkloadOperationPhase,
) -> Option<Vec<WorkloadOperationPhase>> {
    if from == to {
        return Some(Vec::new());
    }
    if matches!(
        to,
        WorkloadOperationPhase::Failed | WorkloadOperationPhase::Cancelled
    ) {
        return Some(vec![to]);
    }
    let mut steps = Vec::new();
    let mut current = from;
    while current != to {
        current = match current {
            WorkloadOperationPhase::Admitting => WorkloadOperationPhase::Defining,
            WorkloadOperationPhase::Defining => WorkloadOperationPhase::Starting,
            WorkloadOperationPhase::Starting => WorkloadOperationPhase::WaitingForGuest,
            WorkloadOperationPhase::WaitingForGuest => WorkloadOperationPhase::WaitingForService,
            WorkloadOperationPhase::WaitingForService => WorkloadOperationPhase::PreparingDisplay,
            WorkloadOperationPhase::PreparingDisplay => {
                WorkloadOperationPhase::WaitingForFirstFrame
            }
            WorkloadOperationPhase::WaitingForFirstFrame => WorkloadOperationPhase::Ready,
            WorkloadOperationPhase::Ready => WorkloadOperationPhase::Completed,
            WorkloadOperationPhase::Stopping => WorkloadOperationPhase::Completed,
            _ => return None,
        };
        steps.push(current);
    }
    Some(steps)
}

fn bounded_reason(reason: &str) -> String {
    reason.chars().take(512).collect()
}

fn safe_request_id(body: &str) -> String {
    let request_id = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("request_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        });
    match request_id {
        Some(request_id)
            if !request_id.is_empty()
                && request_id.len()
                    <= mackes_mesh_types::workloads::MAX_WORKLOAD_IDENTIFIER_BYTES
                && !request_id.chars().any(char::is_control) =>
        {
            request_id
        }
        _ => "invalid-request".to_string(),
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn normalize_qemu_display1_address(raw: &str) -> Result<String, WorkloadActuatorError> {
    let raw = raw.trim();
    let address = raw.strip_prefix("dbus+").unwrap_or(raw);
    if address.is_empty() || address.len() > 4 * 1024 || !address.starts_with("unix:") {
        return Err(WorkloadActuatorError::Permanent(
            "libvirt returned an unsupported QEMU Display1 address".into(),
        ));
    }
    Ok(address.to_owned())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::workloads::{WorkloadId, WorkloadProfile};
    use std::sync::{Arc, Mutex};

    struct AllowAuthorizer;
    impl WorkloadAuthorizer for AllowAuthorizer {
        fn authorize(&self, _: &str, _: &WorkloadOperationRequest, _: i64) -> Result<(), String> {
            Ok(())
        }
    }

    struct FakeActuator {
        calls: Arc<Mutex<u32>>,
    }
    impl WorkloadActuator for FakeActuator {
        fn migration_request_stop(&self, _: &str) -> Result<(), WorkloadActuatorError> {
            *self.calls.lock().expect("calls") += 1;
            Ok(())
        }

        fn apply(
            &self,
            _: &WorkloadOperationRequest,
        ) -> Result<WorkloadActuatorOutcome, WorkloadActuatorError> {
            *self.calls.lock().expect("calls") += 1;
            Ok(WorkloadActuatorOutcome {
                phase: WorkloadOperationPhase::WaitingForGuest,
                power: WorkloadPowerState::Starting,
                readiness: WorkloadReadiness::WaitingForGuest,
                retryable: true,
                reason: None,
                remediation: None,
                attachment: None,
            })
        }
        fn cancel(
            &self,
            _: &WorkloadOperationRequest,
            _: &WorkloadOperationStatus,
        ) -> Result<WorkloadActuatorOutcome, WorkloadActuatorError> {
            *self.calls.lock().expect("calls") += 1;
            Ok(WorkloadActuatorOutcome {
                phase: WorkloadOperationPhase::Cancelled,
                power: WorkloadPowerState::Stopped,
                readiness: WorkloadReadiness::Unavailable,
                retryable: false,
                reason: Some("fake target cleanup complete".into()),
                remediation: None,
                attachment: None,
            })
        }
        fn observe(
            &self,
            _: &WorkloadOperationRequest,
            _: &WorkloadOperationStatus,
        ) -> Result<Option<WorkloadActuatorOutcome>, WorkloadActuatorError> {
            Ok(None)
        }
    }

    struct JournalObservingMigrationActuator {
        calls: Arc<Mutex<u32>>,
        state_root: PathBuf,
        saw_pending_journal: Arc<AtomicBool>,
    }

    impl WorkloadActuator for JournalObservingMigrationActuator {
        fn migration_request_stop(&self, _: &str) -> Result<(), WorkloadActuatorError> {
            let pending = WorkloadMigrationJournal::open(&self.state_root)
                .and_then(|journal| journal.pending())
                .map(|records| {
                    records.iter().any(|record| {
                        record.phase == WorkloadMigrationJournalPhase::Pending
                            && matches!(
                                &record.command,
                                WorkloadMigrationCommand::RequestStop { vm_id }
                                    if vm_id == "vm-reconciler-owned"
                            )
                    })
                })
                .unwrap_or(false);
            self.saw_pending_journal.store(pending, Ordering::Release);
            *self.calls.lock().expect("calls") += 1;
            Ok(())
        }

        fn apply(
            &self,
            _: &WorkloadOperationRequest,
        ) -> Result<WorkloadActuatorOutcome, WorkloadActuatorError> {
            unreachable!("migration journal test does not apply a Workload operation")
        }

        fn cancel(
            &self,
            _: &WorkloadOperationRequest,
            _: &WorkloadOperationStatus,
        ) -> Result<WorkloadActuatorOutcome, WorkloadActuatorError> {
            unreachable!("migration journal test does not cancel a Workload operation")
        }

        fn observe(
            &self,
            _: &WorkloadOperationRequest,
            _: &WorkloadOperationStatus,
        ) -> Result<Option<WorkloadActuatorOutcome>, WorkloadActuatorError> {
            Ok(None)
        }
    }

    #[test]
    fn migration_command_executes_only_when_workload_reconciler_drains() {
        let state = tempfile::tempdir().expect("migration state");
        let calls = Arc::new(Mutex::new(0));
        let saw_pending_journal = Arc::new(AtomicBool::new(false));
        let worker = WorkloadComputeWorker::new("seat15".into(), 1)
            .with_state_root(state.path().to_path_buf())
            .with_actuator(Box::new(JournalObservingMigrationActuator {
                calls: Arc::clone(&calls),
                state_root: state.path().to_path_buf(),
                saw_pending_journal: Arc::clone(&saw_pending_journal),
            }));
        worker.register_migration_executor();
        let request = std::thread::spawn(|| {
            WorkloadMigrationClient
                .request_stop("vm-reconciler-owned")
                .expect("reconciler command")
        });
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(*calls.lock().expect("calls"), 0);
        worker.drain_migration_commands();
        request.join().expect("client join");
        assert_eq!(*calls.lock().expect("calls"), 1);
        assert!(saw_pending_journal.load(Ordering::Acquire));
        assert!(WorkloadMigrationJournal::open(state.path())
            .expect("journal")
            .pending()
            .expect("pending")
            .is_empty());
    }

    #[test]
    fn pending_migration_command_replays_after_worker_restart_without_a_client() {
        let state = tempfile::tempdir().expect("migration state");
        let journal = WorkloadMigrationJournal::open(state.path()).expect("journal");
        journal
            .store(&WorkloadMigrationJournalRecord {
                schema_version: 1,
                command_id: "00000000000000000000000000000001-00000001-0000000000000001".into(),
                phase: WorkloadMigrationJournalPhase::Pending,
                command: WorkloadMigrationCommand::RequestStop {
                    vm_id: "vm-recovered".into(),
                },
            })
            .expect("seed pending command");

        let calls = Arc::new(Mutex::new(0));
        let worker = WorkloadComputeWorker::new("seat15".into(), 1)
            .with_state_root(state.path().to_path_buf())
            .with_actuator(Box::new(FakeActuator {
                calls: Arc::clone(&calls),
            }));
        worker.drain_migration_commands();

        assert_eq!(*calls.lock().expect("calls"), 1);
        assert!(journal
            .pending()
            .expect("pending after recovery")
            .is_empty());
    }

    #[test]
    fn applied_migration_record_is_cleaned_without_repeating_its_effect() {
        let state = tempfile::tempdir().expect("migration state");
        let journal = WorkloadMigrationJournal::open(state.path()).expect("journal");
        journal
            .store(&WorkloadMigrationJournalRecord {
                schema_version: 1,
                command_id: "00000000000000000000000000000002-00000001-0000000000000002".into(),
                phase: WorkloadMigrationJournalPhase::Applied,
                command: WorkloadMigrationCommand::RequestStop {
                    vm_id: "vm-already-applied".into(),
                },
            })
            .expect("seed applied command");

        let calls = Arc::new(Mutex::new(0));
        let worker = WorkloadComputeWorker::new("seat15".into(), 1)
            .with_state_root(state.path().to_path_buf())
            .with_actuator(Box::new(FakeActuator {
                calls: Arc::clone(&calls),
            }));
        worker.drain_migration_commands();

        assert_eq!(*calls.lock().expect("calls"), 0);
        assert!(journal.pending().expect("pending after cleanup").is_empty());
    }

    #[test]
    fn migration_journal_rejects_oversized_definition_and_symlink_root() {
        let state = tempfile::tempdir().expect("migration state");
        let journal = WorkloadMigrationJournal::open(state.path()).expect("journal");
        let oversized = WorkloadMigrationJournalRecord {
            schema_version: 1,
            command_id: "00000000000000000000000000000003-00000001-0000000000000003".into(),
            phase: WorkloadMigrationJournalPhase::Pending,
            command: WorkloadMigrationCommand::DefineAndStart {
                vm_id: "vm-oversized".into(),
                domain_xml: "x".repeat(MAX_MIGRATION_DOMAIN_XML_BYTES + 1),
            },
        };
        assert!(journal.store(&oversized).is_err());

        let duplicate_id = "00000000000000000000000000000004-00000001-0000000000000004";
        fs::write(
            journal.record_path(duplicate_id),
            format!(
                r#"{{"schema_version":1,"schema_version":1,"command_id":"{duplicate_id}","phase":"pending","command":{{"kind":"request_stop","vm_id":"vm-duplicate"}}}}"#
            ),
        )
        .expect("write hostile duplicate record");
        assert!(journal.pending().is_err());

        let hostile = tempfile::tempdir().expect("hostile state");
        std::os::unix::fs::symlink(
            state.path(),
            hostile.path().join(MIGRATION_COMMAND_JOURNAL_DIR),
        )
        .expect("symlink journal root");
        assert!(WorkloadMigrationJournal::open(hostile.path()).is_err());
    }

    struct RetryOnObserveActuator {
        observe_calls: Arc<Mutex<u32>>,
    }

    impl WorkloadActuator for RetryOnObserveActuator {
        fn apply(
            &self,
            _: &WorkloadOperationRequest,
        ) -> Result<WorkloadActuatorOutcome, WorkloadActuatorError> {
            Ok(WorkloadActuatorOutcome {
                phase: WorkloadOperationPhase::WaitingForGuest,
                power: WorkloadPowerState::Starting,
                readiness: WorkloadReadiness::WaitingForGuest,
                retryable: true,
                reason: None,
                remediation: None,
                attachment: None,
            })
        }

        fn cancel(
            &self,
            _: &WorkloadOperationRequest,
            _: &WorkloadOperationStatus,
        ) -> Result<WorkloadActuatorOutcome, WorkloadActuatorError> {
            Ok(WorkloadActuatorOutcome {
                phase: WorkloadOperationPhase::Cancelled,
                power: WorkloadPowerState::Stopped,
                readiness: WorkloadReadiness::Unavailable,
                retryable: false,
                reason: None,
                remediation: None,
                attachment: None,
            })
        }

        fn observe(
            &self,
            _: &WorkloadOperationRequest,
            _: &WorkloadOperationStatus,
        ) -> Result<Option<WorkloadActuatorOutcome>, WorkloadActuatorError> {
            *self.observe_calls.lock().expect("observe calls") += 1;
            Err(WorkloadActuatorError::Retryable(
                "backend is temporarily busy".into(),
            ))
        }
    }

    struct PermanentActuator;
    impl WorkloadActuator for PermanentActuator {
        fn apply(
            &self,
            _: &WorkloadOperationRequest,
        ) -> Result<WorkloadActuatorOutcome, WorkloadActuatorError> {
            Err(WorkloadActuatorError::Permanent(
                "approved VM image is missing".into(),
            ))
        }

        fn cancel(
            &self,
            _: &WorkloadOperationRequest,
            _: &WorkloadOperationStatus,
        ) -> Result<WorkloadActuatorOutcome, WorkloadActuatorError> {
            Err(WorkloadActuatorError::Permanent(
                "approved VM image is missing".into(),
            ))
        }

        fn observe(
            &self,
            _: &WorkloadOperationRequest,
            _: &WorkloadOperationStatus,
        ) -> Result<Option<WorkloadActuatorOutcome>, WorkloadActuatorError> {
            Err(WorkloadActuatorError::Permanent(
                "approved VM image is missing".into(),
            ))
        }
    }

    struct PrematureCompleteActuator;
    impl WorkloadActuator for PrematureCompleteActuator {
        fn apply(
            &self,
            _: &WorkloadOperationRequest,
        ) -> Result<WorkloadActuatorOutcome, WorkloadActuatorError> {
            Ok(WorkloadActuatorOutcome {
                phase: WorkloadOperationPhase::WaitingForGuest,
                power: WorkloadPowerState::Starting,
                readiness: WorkloadReadiness::WaitingForGuest,
                retryable: true,
                reason: None,
                remediation: None,
                attachment: None,
            })
        }

        fn cancel(
            &self,
            _: &WorkloadOperationRequest,
            _: &WorkloadOperationStatus,
        ) -> Result<WorkloadActuatorOutcome, WorkloadActuatorError> {
            Ok(WorkloadActuatorOutcome {
                phase: WorkloadOperationPhase::Cancelled,
                power: WorkloadPowerState::Stopped,
                readiness: WorkloadReadiness::Unavailable,
                retryable: false,
                reason: Some("fake target cleanup complete".into()),
                remediation: None,
                attachment: None,
            })
        }

        fn observe(
            &self,
            _: &WorkloadOperationRequest,
            _: &WorkloadOperationStatus,
        ) -> Result<Option<WorkloadActuatorOutcome>, WorkloadActuatorError> {
            Ok(Some(WorkloadActuatorOutcome {
                phase: WorkloadOperationPhase::Completed,
                power: WorkloadPowerState::Running,
                readiness: WorkloadReadiness::Ready,
                retryable: false,
                reason: None,
                remediation: None,
                attachment: None,
            }))
        }
    }

    struct RecoveryActuator {
        calls: Arc<Mutex<u32>>,
        revoked: Arc<Mutex<Vec<(String, u64)>>>,
        outcome: WorkloadActuatorOutcome,
    }

    impl WorkloadActuator for RecoveryActuator {
        fn apply(
            &self,
            _: &WorkloadOperationRequest,
        ) -> Result<WorkloadActuatorOutcome, WorkloadActuatorError> {
            unreachable!("terminal recovery must not apply a lifecycle operation")
        }

        fn cancel(
            &self,
            _: &WorkloadOperationRequest,
            _: &WorkloadOperationStatus,
        ) -> Result<WorkloadActuatorOutcome, WorkloadActuatorError> {
            unreachable!("terminal recovery must not cancel a lifecycle operation")
        }

        fn observe(
            &self,
            _: &WorkloadOperationRequest,
            _: &WorkloadOperationStatus,
        ) -> Result<Option<WorkloadActuatorOutcome>, WorkloadActuatorError> {
            unreachable!("terminal recovery must not enter inflight observation")
        }

        fn recover_attachment(
            &self,
            _: &WorkloadOperationRequest,
            _: &WorkloadOperationStatus,
            _: u64,
        ) -> Result<Option<WorkloadActuatorOutcome>, WorkloadActuatorError> {
            *self.calls.lock().expect("recovery calls") += 1;
            Ok(Some(self.outcome.clone()))
        }

        fn revoke_attachment(&self, status: &WorkloadOperationStatus) {
            if let Some(lease) = status.attachment.as_ref() {
                self.revoked
                    .lock()
                    .expect("revoked attachments")
                    .push((lease.lease_id.clone(), lease.generation));
            }
        }
    }

    fn request() -> WorkloadOperationRequest {
        WorkloadOperationRequest {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            request_id: "op-1".into(),
            workload_id: WorkloadId::new("browser-seat15").expect("id"),
            backend: WorkloadBackend::LibvirtVirtqemud,
            resources: WorkloadProfile::Small.resources(),
            image_ref: None,
            target_node: "seat15".into(),
            expected_generation: 0,
            action: WorkloadOperationAction::StartAndAttach,
            target_request_id: None,
            deadline_at_ms: now_ms() + 20_000,
            preferred_attachment: None,
            armed_token: None,
        }
    }

    fn seed_completed_attachment(
        ledger: &mut WorkloadOperationLedger,
        request: WorkloadOperationRequest,
        lease: WorkloadAttachmentLease,
        now: u64,
    ) {
        let mut status = ledger
            .accept(request.clone(), now)
            .expect("queue recovery record");
        for phase in [
            WorkloadOperationPhase::Validating,
            WorkloadOperationPhase::Admitting,
            WorkloadOperationPhase::Defining,
            WorkloadOperationPhase::Starting,
            WorkloadOperationPhase::WaitingForGuest,
            WorkloadOperationPhase::WaitingForService,
            WorkloadOperationPhase::PreparingDisplay,
            WorkloadOperationPhase::WaitingForFirstFrame,
            WorkloadOperationPhase::Ready,
            WorkloadOperationPhase::Completed,
        ] {
            status.phase = phase;
            if phase == WorkloadOperationPhase::Completed {
                status.power = WorkloadPowerState::Running;
                status.readiness = WorkloadReadiness::Ready;
                status.signals = WorkloadRuntimeSignals::from_readiness(phase, status.readiness);
                status.attachment = Some(lease.clone());
            }
            status = ledger
                .advance(&request.request_id, status, now)
                .expect("advance recovery record");
        }
    }

    fn test_capacity() -> HostCapacity {
        HostCapacity {
            logical_cpus: 4,
            memory_mb: 16_384,
            allocated_vcpu: 0,
            allocated_memory_mb: 0,
            storage_gb: 128,
            allocated_storage_gb: 0,
        }
    }

    #[test]
    fn container_admission_uses_container_pool_not_vm_pool() {
        let temp = tempfile::tempdir().expect("temp");
        let calls = Arc::new(Mutex::new(0));
        let mut worker = WorkloadComputeWorker::new("seat15".into(), 1)
            .with_state_root(temp.path().to_path_buf())
            .with_authorizer(Box::new(AllowAuthorizer))
            .with_capacity(test_capacity())
            .with_storage_capacity(WorkloadStorageCapacity {
                vm_storage_gb: 128,
                allocated_vm_storage_gb: 0,
                container_storage_gb: 0,
                allocated_container_storage_gb: 0,
            })
            .with_actuator(Box::new(FakeActuator {
                calls: calls.clone(),
            }));
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("ledger");
        let mut request = request();
        request.backend = WorkloadBackend::QuadletSystemd;
        request.workload_id = WorkloadId::new("container:seat15:mesh-api").expect("id");
        let raw = serde_json::to_string(&request).expect("wire");

        worker.handle_request(&mut ledger, &raw, request, now_ms());
        let status = ledger.status("op-1").expect("status");
        assert_eq!(status.phase, WorkloadOperationPhase::Failed);
        assert_eq!(*calls.lock().expect("calls"), 0);
        assert!(status
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("managed storage pool")));
    }

    #[test]
    fn stable_source_identity_maps_to_the_libvirt_domain_name() {
        let mut request = request();
        request.workload_id = WorkloadId::new("vm:seat15:browser").expect("id");
        assert_eq!(SystemWorkloadActuator::libvirt_domain(&request), "browser");
        assert_eq!(
            SystemWorkloadActuator::vm_overlay_path(&request),
            Path::new("/var/lib/mde-vms/browser.qcow2")
        );
    }

    #[test]
    fn libvirt_cleanup_treats_absent_and_stopped_domains_as_idempotent() {
        assert!(libvirt_domain_absent_or_stopped("error: Domain not found"));
        assert!(libvirt_domain_absent_or_stopped(
            "Requested operation is not valid: domain is not running"
        ));
        assert!(libvirt_domain_absent_or_stopped(
            "failed to get domain 'browser'"
        ));
        assert!(!libvirt_domain_absent_or_stopped(
            "permission denied while contacting virtqemud"
        ));
        assert!(libvirt_domain_absent("error: Domain not found"));
        assert!(!libvirt_domain_absent(
            "Requested operation is not valid: domain is not running"
        ));
        assert!(libvirt_domain_already_running(
            "Requested operation is not valid: domain is already active"
        ));
        assert!(!libvirt_domain_already_running(
            "permission denied while contacting virtqemud"
        ));
    }

    #[test]
    fn display1_lease_is_deterministic_and_tracks_next_generation() {
        let request = request();
        let now = now_ms();
        let first = SystemWorkloadActuator::attachment_lease(&request, 1, now);
        let repeat = SystemWorkloadActuator::attachment_lease(&request, 1, now);
        assert_eq!(first, repeat);
        assert_eq!(first.generation, 1);
        assert!(first.lease_id.starts_with("display1-"));
        assert!(first.validate(now).is_ok());

        let mut next_request = request;
        next_request.expected_generation = 1;
        let next = SystemWorkloadActuator::attachment_lease(&next_request, 2, now);
        assert_ne!(first.lease_id, next.lease_id);
        assert_eq!(next.generation, 2);
    }

    #[test]
    fn qemu_display1_address_normalization_rejects_network_endpoints() {
        assert_eq!(
            normalize_qemu_display1_address("dbus+unix:path=/run/libvirt/qemu/dbus.sock\n")
                .expect("unix address"),
            "unix:path=/run/libvirt/qemu/dbus.sock"
        );
        assert!(matches!(
            normalize_qemu_display1_address("tcp:host=127.0.0.1,port=1234"),
            Err(WorkloadActuatorError::Permanent(_))
        ));
    }

    #[test]
    fn actuator_starts_and_reuses_a_node_local_display1_server() {
        let temp = tempfile::tempdir().expect("temp");
        let actuator = SystemWorkloadActuator::new(temp.path().join("state"))
            .with_display1_root(temp.path().join("display1"));
        let request = request();
        let runtime = actuator
            .ensure_attachment(&request, 1, now_ms())
            .expect("server");
        let socket = runtime.server.socket_path().to_path_buf();
        assert!(socket.exists());
        let same = actuator
            .ensure_attachment(&request, 1, now_ms())
            .expect("reuse");
        assert!(Arc::ptr_eq(&runtime, &same));
        drop(same);
        drop(runtime);
        drop(actuator);
        assert!(!socket.exists());
    }

    #[test]
    fn expired_persisted_display1_lease_is_replaced_during_recovery() {
        let temp = tempfile::tempdir().expect("temp");
        let now = now_ms();
        let request = request();
        let mut status = queued_status(&request);
        let mut expired = SystemWorkloadActuator::attachment_lease(&request, 1, now);
        expired.expires_at_ms = now.saturating_sub(1);
        status.attachment = Some(expired);
        let actuator = SystemWorkloadActuator::new(temp.path().to_path_buf())
            .with_display1_root(temp.path().join("display1"));

        let runtime = actuator
            .attachment_for_status(&request, &status, now)
            .expect("expired lease is recoverable");
        assert_eq!(runtime.server.lease().generation, status.generation);
        assert!(runtime.server.lease().expires_at_ms > now);
        assert_ne!(runtime.server.lease(), status.attachment.as_ref().unwrap());
    }

    #[test]
    fn old_generation_is_revoked_while_latest_exact_attachment_reconciles() {
        let temp = tempfile::tempdir().expect("temp");
        let now = now_ms();
        let old_request = request();
        let old_lease = SystemWorkloadActuator::attachment_lease(&old_request, 1, now);
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("ledger");
        seed_completed_attachment(&mut ledger, old_request.clone(), old_lease.clone(), now);
        let mut request = request();
        request.request_id = "op-2".into();
        request.expected_generation = 1;
        let lease = SystemWorkloadActuator::attachment_lease(&request, 2, now);
        seed_completed_attachment(&mut ledger, request.clone(), lease.clone(), now);
        let calls = Arc::new(Mutex::new(0));
        let revoked = Arc::new(Mutex::new(Vec::new()));
        let mut worker = WorkloadComputeWorker::new("seat15".into(), 1).with_actuator(Box::new(
            RecoveryActuator {
                calls: Arc::clone(&calls),
                revoked: Arc::clone(&revoked),
                outcome: WorkloadActuatorOutcome {
                    phase: WorkloadOperationPhase::Completed,
                    power: WorkloadPowerState::Running,
                    readiness: WorkloadReadiness::PreparingDisplay,
                    retryable: true,
                    reason: Some("waiting for recovered first frame".into()),
                    remediation: Some("keep the shell attached".into()),
                    attachment: Some(lease.clone()),
                },
            },
        ));

        worker.reconcile_recovered_attachments(&mut ledger, now);

        assert_eq!(*calls.lock().expect("recovery calls"), 1);
        assert_eq!(
            *revoked.lock().expect("revoked attachments"),
            vec![(old_lease.lease_id.clone(), 1)]
        );
        let stale = ledger
            .status(&old_request.request_id)
            .expect("superseded status");
        assert!(stale.attachment.is_none());
        assert_eq!(stale.readiness, WorkloadReadiness::Unavailable);
        assert!(stale
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("superseded")));
        let recovered = ledger
            .status(&request.request_id)
            .expect("recovered status");
        assert_eq!(recovered.phase, WorkloadOperationPhase::Completed);
        assert_eq!(recovered.readiness, WorkloadReadiness::PreparingDisplay);
        assert_eq!(recovered.attachment.as_ref(), Some(&lease));
        assert!(recovered
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("recovered first frame")));
    }

    #[test]
    fn recovered_attachment_with_wrong_generation_is_refused_and_unpublished() {
        let temp = tempfile::tempdir().expect("temp");
        let now = now_ms();
        let request = request();
        let lease = SystemWorkloadActuator::attachment_lease(&request, 1, now);
        let mut hostile = lease.clone();
        hostile.generation = 2;
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("ledger");
        seed_completed_attachment(&mut ledger, request.clone(), lease, now);
        let mut worker = WorkloadComputeWorker::new("seat15".into(), 1).with_actuator(Box::new(
            RecoveryActuator {
                calls: Arc::new(Mutex::new(0)),
                revoked: Arc::new(Mutex::new(Vec::new())),
                outcome: WorkloadActuatorOutcome {
                    phase: WorkloadOperationPhase::Completed,
                    power: WorkloadPowerState::Running,
                    readiness: WorkloadReadiness::Ready,
                    retryable: false,
                    reason: None,
                    remediation: None,
                    attachment: Some(hostile),
                },
            },
        ));

        worker.reconcile_recovered_attachments(&mut ledger, now);

        let refused = ledger.status(&request.request_id).expect("refused status");
        assert!(refused.attachment.is_none());
        assert_eq!(refused.readiness, WorkloadReadiness::Unavailable);
        assert!(refused
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("mismatched identity")));
        assert!(refused
            .remediation
            .as_deref()
            .is_some_and(|remediation| remediation.contains("current generation")));
    }

    #[test]
    fn revoking_stale_recovered_lease_removes_its_node_local_socket() {
        let temp = tempfile::tempdir().expect("temp");
        let display_root = temp.path().join("display1");
        fs::create_dir_all(&display_root).expect("display root");
        let request = request();
        let lease = SystemWorkloadActuator::attachment_lease(&request, 1, now_ms());
        let socket = display1_socket_path_at(&display_root, &lease.lease_id).expect("socket path");
        fs::write(&socket, b"stale socket placeholder").expect("stale socket");
        let actuator =
            SystemWorkloadActuator::new(temp.path().join("state")).with_display1_root(display_root);
        let mut status = queued_status(&request);
        status.attachment = Some(lease);

        actuator.revoke_persisted_attachment(&status);

        assert!(!socket.exists());
        assert!(actuator.attachments.lock().expect("attachments").is_empty());
    }

    #[test]
    fn expired_display1_runtime_is_reaped_and_socket_is_removed() {
        let temp = tempfile::tempdir().expect("temp");
        let display_root = temp.path().join("display1");
        let actuator =
            SystemWorkloadActuator::new(temp.path().join("state")).with_display1_root(display_root);
        let runtime = actuator
            .ensure_attachment(&request(), 1, now_ms())
            .expect("server");
        let socket = runtime.server.socket_path().to_path_buf();
        let expires_at_ms = runtime.server.lease().expires_at_ms;
        assert!(socket.exists());

        drop(runtime);
        actuator.reap_expired(expires_at_ms);

        assert!(!socket.exists());
        assert!(actuator.attachments.lock().expect("attachments").is_empty());
    }

    #[test]
    fn stopped_workload_releases_display1_runtime_immediately() {
        let temp = tempfile::tempdir().expect("temp");
        let actuator = SystemWorkloadActuator::new(temp.path().join("state"))
            .with_display1_root(temp.path().join("display1"));
        let request = request();
        let runtime = actuator
            .ensure_attachment(&request, 1, now_ms())
            .expect("server");
        let socket = runtime.server.socket_path().to_path_buf();
        drop(runtime);
        assert!(socket.exists());

        let outcome = actuator.stopped_outcome(&request);

        assert!(!socket.exists());
        assert!(actuator.attachments.lock().expect("attachments").is_empty());
        assert_eq!(outcome.phase, WorkloadOperationPhase::Completed);
        assert_eq!(outcome.power, WorkloadPowerState::Stopped);
        assert_eq!(outcome.readiness, WorkloadReadiness::Unavailable);
    }

    #[test]
    fn cancel_never_creates_display1_server_or_invokes_vm_side_effects() {
        let temp = tempfile::tempdir().expect("temp");
        let display_root = temp.path().join("display1");
        let actuator = SystemWorkloadActuator::new(temp.path().join("state"))
            .with_display1_root(display_root.clone());
        let mut request = request();
        request.action = WorkloadOperationAction::Cancel;

        let outcome = actuator.apply(&request).expect("cancel outcome");

        assert_eq!(outcome.phase, WorkloadOperationPhase::Cancelled);
        assert_eq!(outcome.power, WorkloadPowerState::Stopped);
        assert_eq!(outcome.readiness, WorkloadReadiness::Unavailable);
        assert!(!display_root.exists());
    }

    #[test]
    fn approved_image_resolution_requires_exact_promoted_vm_artifact() {
        let temp = tempfile::tempdir().expect("temp");
        let version_dir = crate::image_catalog::images_dir(temp.path())
            .join("fedora")
            .join("1.0");
        std::fs::create_dir_all(&version_dir).expect("image dir");
        std::fs::write(
            version_dir.join("manifest.toml"),
            "name = \"fedora\"\nkind = \"vm\"\nversion = \"1.0\"\n",
        )
        .expect("manifest");
        std::fs::write(version_dir.join("fedora.img"), b"qcow2").expect("artifact");
        std::fs::write(
            crate::image_catalog::images_dir(temp.path())
                .join("fedora")
                .join("PROMOTED"),
            "1.0\n",
        )
        .expect("promotion");
        let actuator = SystemWorkloadActuator::new(temp.path().to_path_buf());
        assert_eq!(
            actuator
                .approved_image("fedora:1.0")
                .expect("approved image"),
            version_dir.join("fedora.img")
        );
        assert!(actuator.approved_image("fedora:2.0").is_err());
        assert!(actuator.approved_image("../fedora:1.0").is_err());
    }

    #[test]
    fn approved_container_resolution_requires_promoted_nonempty_oci_artifact() {
        let temp = tempfile::tempdir().expect("temp");
        let version_dir = crate::image_catalog::images_dir(temp.path())
            .join("mesh-api")
            .join("1.0");
        std::fs::create_dir_all(&version_dir).expect("image dir");
        std::fs::write(
            version_dir.join("manifest.toml"),
            "name = \"mesh-api\"\nkind = \"container\"\nversion = \"1.0\"\n",
        )
        .expect("manifest");
        let artifact = version_dir.join("mesh-api-1.0.oci.tar");
        std::fs::write(&artifact, b"oci archive").expect("artifact");
        std::fs::write(
            crate::image_catalog::images_dir(temp.path())
                .join("mesh-api")
                .join("PROMOTED"),
            "1.0\n",
        )
        .expect("promotion");

        let actuator = SystemWorkloadActuator::new(temp.path().to_path_buf());
        assert_eq!(
            actuator
                .approved_container_image("mesh-api:1.0")
                .expect("approved container"),
            artifact
        );

        std::fs::write(&artifact, []).expect("empty artifact");
        assert!(actuator
            .approved_container_image("mesh-api:1.0")
            .expect_err("empty artifact must fail")
            .contains("empty"));
        assert!(actuator.approved_container_image("mesh-api:2.0").is_err());
    }

    #[test]
    fn quadlet_materialization_is_tied_to_typed_workload_identity() {
        let mut request = request();
        request.backend = WorkloadBackend::QuadletSystemd;
        request.workload_id =
            WorkloadId::new("container:seat15:mesh-api").expect("typed workload id");
        request.image_ref = Some("mesh-api:1.0".into());

        let unit = SystemWorkloadActuator::render_quadlet(
            &request,
            request.image_ref.as_deref().expect("image ref"),
        );
        assert!(unit.contains("Description=MCNF workload container:seat15:mesh-api"));
        assert!(unit.contains("Image=mesh-api:1.0"));
        assert!(unit.contains("ContainerName=mde-workload-container-seat15-mesh-api-"));
        assert!(!unit.contains("ContainerName=mde-workload-container:seat15:mesh-api"));
        assert!(SystemWorkloadActuator::runtime_name(&request)
            .chars()
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, '-' | '_' | '.')));
        assert_eq!(
            SystemWorkloadActuator::quadlet_unit_path(&request),
            Path::new(QUADLET_RUNTIME_ROOT).join(format!(
                "{}.container",
                SystemWorkloadActuator::runtime_name(&request)
            ))
        );
        assert_eq!(
            SystemWorkloadActuator::unit_name(&request),
            format!("{}.service", SystemWorkloadActuator::runtime_name(&request))
        );
    }

    #[cfg(unix)]
    #[test]
    fn approved_container_resolution_rejects_symlinked_artifact() {
        let temp = tempfile::tempdir().expect("temp");
        let version_dir = crate::image_catalog::images_dir(temp.path())
            .join("mesh-api")
            .join("1.0");
        std::fs::create_dir_all(&version_dir).expect("image dir");
        std::fs::write(
            version_dir.join("manifest.toml"),
            "name = \"mesh-api\"\nkind = \"container\"\nversion = \"1.0\"\n",
        )
        .expect("manifest");
        std::fs::write(
            crate::image_catalog::images_dir(temp.path())
                .join("mesh-api")
                .join("PROMOTED"),
            "1.0\n",
        )
        .expect("promotion");
        let outside = temp.path().join("outside.oci.tar");
        std::fs::write(&outside, b"outside").expect("outside artifact");
        std::os::unix::fs::symlink(&outside, version_dir.join("mesh-api-1.0.oci.tar"))
            .expect("symlink");

        let actuator = SystemWorkloadActuator::new(temp.path().to_path_buf());
        assert!(actuator
            .approved_container_image("mesh-api:1.0")
            .expect_err("symlink must fail")
            .contains("regular file"));
    }

    #[test]
    fn authorized_request_is_journaled_before_fake_actuator_and_replays() {
        let temp = tempfile::tempdir().expect("temp");
        let calls = Arc::new(Mutex::new(0));
        let mut worker = WorkloadComputeWorker::new("seat15".into(), 1)
            .with_state_root(temp.path().to_path_buf())
            .with_authorizer(Box::new(AllowAuthorizer))
            .with_capacity(test_capacity())
            .with_actuator(Box::new(FakeActuator {
                calls: calls.clone(),
            }));
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("ledger");
        let request = request();
        let raw = serde_json::to_string(&request).expect("wire");
        worker.handle_request(&mut ledger, &raw, request.clone(), now_ms());
        worker.handle_request(&mut ledger, &raw, request, now_ms());
        assert_eq!(*calls.lock().expect("calls"), 1);
        assert_eq!(
            ledger.status("op-1").expect("status").phase,
            WorkloadOperationPhase::WaitingForGuest
        );
    }

    #[test]
    fn queued_cancel_is_terminal_without_calling_the_actuator() {
        let temp = tempfile::tempdir().expect("temp");
        let calls = Arc::new(Mutex::new(0));
        let mut worker = WorkloadComputeWorker::new("seat15".into(), 1)
            .with_state_root(temp.path().to_path_buf())
            .with_authorizer(Box::new(AllowAuthorizer))
            .with_capacity(test_capacity())
            .with_actuator(Box::new(FakeActuator {
                calls: calls.clone(),
            }));
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("ledger");
        let mut target = request();
        target.request_id = "target-1".into();
        ledger.accept(target, now_ms()).expect("target queued");
        let mut request = request();
        request.request_id = "cancel-1".into();
        request.action = WorkloadOperationAction::Cancel;
        request.expected_generation = 1;
        request.target_request_id = Some("target-1".into());
        let raw = serde_json::to_string(&request).expect("wire");

        worker.handle_request(&mut ledger, &raw, request.clone(), now_ms());

        let status = ledger.status(&request.request_id).expect("status");
        assert_eq!(status.phase, WorkloadOperationPhase::Completed);
        assert_eq!(status.power, WorkloadPowerState::Stopped);
        assert_eq!(status.readiness, WorkloadReadiness::Unavailable);
        assert_eq!(
            ledger.status("target-1").expect("target").phase,
            WorkloadOperationPhase::Cancelled
        );
        assert_eq!(*calls.lock().expect("calls"), 0);
    }

    #[test]
    fn running_cancel_targets_the_journaled_operation_and_cleans_the_adapter() {
        let temp = tempfile::tempdir().expect("temp");
        let calls = Arc::new(Mutex::new(0));
        let mut worker = WorkloadComputeWorker::new("seat15".into(), 1)
            .with_state_root(temp.path().to_path_buf())
            .with_authorizer(Box::new(AllowAuthorizer))
            .with_capacity(test_capacity())
            .with_actuator(Box::new(FakeActuator {
                calls: calls.clone(),
            }));
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("ledger");
        let mut target = request();
        target.request_id = "target-running".into();
        let target_raw = serde_json::to_string(&target).expect("target wire");
        worker.handle_request(&mut ledger, &target_raw, target.clone(), now_ms());
        assert_eq!(*calls.lock().expect("calls"), 1);
        let target_status = ledger.status("target-running").expect("target status");
        assert_eq!(target_status.phase, WorkloadOperationPhase::WaitingForGuest);

        let mut cancel = request();
        cancel.request_id = "cancel-running".into();
        cancel.action = WorkloadOperationAction::Cancel;
        cancel.expected_generation = target_status.generation;
        cancel.target_request_id = Some(target.request_id.clone());
        let cancel_raw = serde_json::to_string(&cancel).expect("cancel wire");
        worker.handle_request(&mut ledger, &cancel_raw, cancel.clone(), now_ms());

        assert_eq!(*calls.lock().expect("calls"), 2);
        assert_eq!(
            ledger
                .status("target-running")
                .expect("target status")
                .phase,
            WorkloadOperationPhase::Cancelled
        );
        assert_eq!(
            ledger
                .status("cancel-running")
                .expect("cancel status")
                .phase,
            WorkloadOperationPhase::Completed
        );
    }

    #[test]
    fn operation_requests_receive_typed_correlated_replies() {
        let temp = tempfile::tempdir().expect("temp");
        let bus_root = temp.path().join("bus");
        let mut persist = Persist::open(bus_root).expect("bus");
        let calls = Arc::new(Mutex::new(0));
        let mut worker = WorkloadComputeWorker::new("seat15".into(), 1)
            .with_state_root(temp.path().join("state"))
            .with_authorizer(Box::new(AllowAuthorizer))
            .with_capacity(test_capacity())
            .with_actuator(Box::new(FakeActuator {
                calls: calls.clone(),
            }));
        let mut ledger = WorkloadOperationLedger::open(temp.path().join("state")).expect("ledger");
        let request = request();
        let raw = serde_json::to_string(&request).expect("wire");
        let action = persist
            .write(ACTION_TOPIC, Priority::Default, None, Some(&raw))
            .expect("action");

        worker.tick_once(&mut ledger, Some(&mut persist));

        let replies = persist
            .list_since(&reply_topic(&action.ulid), None)
            .expect("replies");
        assert_eq!(replies.len(), 1);
        let reply: WorkloadOperationReply =
            serde_json::from_str(replies[0].body.as_deref().expect("body")).expect("typed");
        assert!(reply.accepted);
        assert_eq!(reply.request_id, request.request_id);
        assert_eq!(reply.error_code, None);
        assert_eq!(
            reply.status.expect("status").phase,
            WorkloadOperationPhase::WaitingForGuest
        );
        assert_eq!(*calls.lock().expect("calls"), 1);

        let malformed = persist
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(r#"{"request_id":"bad","unknown":true}"#),
            )
            .expect("malformed action");
        worker.tick_once(&mut ledger, Some(&mut persist));
        let malformed_reply = persist
            .list_since(&reply_topic(&malformed.ulid), None)
            .expect("malformed reply")
            .into_iter()
            .next()
            .expect("reply");
        let malformed: WorkloadOperationReply =
            serde_json::from_str(malformed_reply.body.as_deref().expect("body"))
                .expect("typed malformed reply");
        assert!(!malformed.accepted);
        assert_eq!(
            malformed.error_code,
            Some(WorkloadOperationErrorCode::MalformedRequest)
        );
        assert_eq!(*calls.lock().expect("calls"), 1);
    }

    #[test]
    fn action_recovery_reads_a_bounded_page_and_advances_the_cursor() {
        let temp = tempfile::tempdir().expect("temp");
        let bus_root = temp.path().join("bus");
        let mut persist = Persist::open(bus_root).expect("bus");
        let mut worker = WorkloadComputeWorker::new("seat15".into(), 1)
            .with_state_root(temp.path().join("state"))
            .with_authorizer(Box::new(AllowAuthorizer))
            .with_capacity(test_capacity());
        let mut ledger = WorkloadOperationLedger::open(temp.path().join("state")).expect("ledger");
        let mut actions = Vec::new();
        for index in 0..(MAX_OPERATION_MESSAGES_PER_TICK + 1) {
            actions.push(
                persist
                    .write(
                        ACTION_TOPIC,
                        Priority::Default,
                        None,
                        Some(&format!(r#"{{"request_id":"bad-{index}","unknown":true}}"#)),
                    )
                    .expect("action"),
            );
        }

        worker.tick_once(&mut ledger, Some(&mut persist));
        let page_last = MAX_OPERATION_MESSAGES_PER_TICK - 1;
        assert_eq!(
            worker.cursor.as_deref(),
            Some(actions[page_last].ulid.as_str())
        );
        assert_eq!(
            persist
                .list_since(&reply_topic(&actions[page_last].ulid), None)
                .expect("reply")
                .len(),
            1
        );
        assert!(persist
            .list_since(
                &reply_topic(&actions[MAX_OPERATION_MESSAGES_PER_TICK].ulid),
                None
            )
            .expect("reply")
            .is_empty());

        worker.tick_once(&mut ledger, Some(&mut persist));
        assert_eq!(
            worker.cursor.as_deref(),
            Some(actions[MAX_OPERATION_MESSAGES_PER_TICK].ulid.as_str())
        );
        assert_eq!(
            persist
                .list_since(
                    &reply_topic(&actions[MAX_OPERATION_MESSAGES_PER_TICK].ulid),
                    None
                )
                .expect("reply")
                .len(),
            1
        );
    }

    #[test]
    fn lighthouse_rejects_workload_before_adapter() {
        let temp = tempfile::tempdir().expect("temp");
        let calls = Arc::new(Mutex::new(0));
        let mut worker = WorkloadComputeWorker::new("lh-1".into(), 0)
            .with_authorizer(Box::new(AllowAuthorizer))
            .with_capacity(test_capacity())
            .with_actuator(Box::new(FakeActuator {
                calls: calls.clone(),
            }));
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("ledger");
        let mut request = request();
        request.target_node = "lh-1".into();
        let raw = serde_json::to_string(&request).expect("wire");
        worker.handle_request(&mut ledger, &raw, request, now_ms());
        let status = ledger.status("op-1").expect("status");
        assert_eq!(status.phase, WorkloadOperationPhase::Failed);
        assert_eq!(*calls.lock().expect("calls"), 0);
        assert!(status
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("lighthouse"));
    }

    #[test]
    fn lighthouse_rejects_every_workload_action_and_backend_including_android() {
        let temp = tempfile::tempdir().expect("temp");
        let calls = Arc::new(Mutex::new(0));
        let mut worker =
            WorkloadComputeWorker::new("lh-1".into(), mde_role::Role::Lighthouse.rank())
                .with_authorizer(Box::new(AllowAuthorizer))
                .with_capacity(test_capacity())
                .with_actuator(Box::new(FakeActuator {
                    calls: calls.clone(),
                }));
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("ledger");
        let actions = [
            WorkloadOperationAction::StartAndAttach,
            WorkloadOperationAction::Start,
            WorkloadOperationAction::Stop,
            WorkloadOperationAction::Restart,
            WorkloadOperationAction::Destroy,
            WorkloadOperationAction::Pause,
            WorkloadOperationAction::Resume,
            WorkloadOperationAction::Open,
            WorkloadOperationAction::Reconcile,
            WorkloadOperationAction::Cancel,
        ];
        let backends = [
            WorkloadBackend::LibvirtVirtqemud,
            WorkloadBackend::QuadletSystemd,
        ];

        for (index, (action, backend)) in actions
            .into_iter()
            .flat_map(|action| backends.into_iter().map(move |backend| (action, backend)))
            .enumerate()
        {
            let mut request = request();
            request.request_id = format!("lighthouse-op-{index}");
            request.workload_id = WorkloadId::new(format!("android-lighthouse-{index}"))
                .expect("Android workload id");
            request.target_node = "lh-1".into();
            request.action = action;
            request.backend = backend;
            if action == WorkloadOperationAction::Cancel {
                let target_id = format!("lighthouse-target-{index}");
                let mut target = request.clone();
                target.request_id = target_id.clone();
                target.action = WorkloadOperationAction::Start;
                target.target_request_id = None;
                target.expected_generation = 0;
                ledger
                    .accept(target, now_ms())
                    .expect("cancellation target");
                request.target_request_id = Some(target_id);
                request.expected_generation = 1;
            }
            let raw = serde_json::to_string(&request).expect("wire");
            worker.handle_request(&mut ledger, &raw, request.clone(), now_ms());

            let status = ledger.status(&request.request_id).expect("rejected status");
            assert_eq!(status.phase, WorkloadOperationPhase::Failed);
            assert!(status
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("lighthouse")));
        }

        assert_eq!(
            *calls.lock().expect("calls"),
            0,
            "a Lighthouse must not actuate any VM, container, or Android workload action"
        );
    }

    #[test]
    fn unknown_role_rank_fails_closed_before_adapter() {
        let temp = tempfile::tempdir().expect("temp");
        let calls = Arc::new(Mutex::new(0));
        let mut worker = WorkloadComputeWorker::new("unknown-1".into(), 99)
            .with_authorizer(Box::new(AllowAuthorizer))
            .with_capacity(test_capacity())
            .with_actuator(Box::new(FakeActuator {
                calls: calls.clone(),
            }));
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("ledger");
        let mut request = request();
        request.target_node = "unknown-1".into();
        let raw = serde_json::to_string(&request).expect("wire");
        worker.handle_request(&mut ledger, &raw, request, now_ms());

        let status = ledger.status("op-1").expect("rejected status");
        assert_eq!(status.phase, WorkloadOperationPhase::Failed);
        assert_eq!(*calls.lock().expect("calls"), 0);
        assert!(status
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("unrecognized")));
    }

    #[test]
    fn queued_replay_after_restart_drives_one_side_effect() {
        let temp = tempfile::tempdir().expect("temp");
        let calls = Arc::new(Mutex::new(0));
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("ledger");
        let request = request();
        ledger.accept(request.clone(), now_ms()).expect("queue");
        let mut worker = WorkloadComputeWorker::new("seat15".into(), 1)
            .with_state_root(temp.path().to_path_buf())
            .with_authorizer(Box::new(AllowAuthorizer))
            .with_capacity(test_capacity())
            .with_actuator(Box::new(FakeActuator {
                calls: calls.clone(),
            }));
        worker.reconcile_inflight(&mut ledger, now_ms());
        assert_eq!(*calls.lock().expect("calls"), 1);
        assert_eq!(
            ledger.status("op-1").expect("status").phase,
            WorkloadOperationPhase::WaitingForGuest
        );
    }

    #[test]
    fn retryable_observation_honors_durable_backoff_after_restart() {
        let temp = tempfile::tempdir().expect("temp");
        let observe_calls = Arc::new(Mutex::new(0));
        let start = now_ms();
        let mut request = request();
        request.deadline_at_ms = start + 20_000;
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("ledger");
        ledger.accept(request.clone(), start).expect("queue");
        let mut worker = WorkloadComputeWorker::new("seat15".into(), 1)
            .with_state_root(temp.path().to_path_buf())
            .with_authorizer(Box::new(AllowAuthorizer))
            .with_capacity(test_capacity())
            .with_actuator(Box::new(RetryOnObserveActuator {
                observe_calls: observe_calls.clone(),
            }));

        // The first recovery pass crosses the journaled defining boundary and
        // applies the operation; the next poll is the first observation.
        worker.reconcile_inflight(&mut ledger, start);
        worker.reconcile_inflight(&mut ledger, start);
        assert_eq!(*observe_calls.lock().expect("observe calls"), 1);
        let retry_at = ledger.status("op-1").expect("status").next_retry_at_ms;
        assert!(retry_at > start);

        worker.reconcile_inflight(&mut ledger, retry_at.saturating_sub(1));
        assert_eq!(
            *observe_calls.lock().expect("observe calls"),
            1,
            "a restart/poll tick before the durable backoff must not re-observe"
        );

        worker.reconcile_inflight(&mut ledger, retry_at);
        assert_eq!(*observe_calls.lock().expect("observe calls"), 2);
    }

    #[test]
    fn permanent_adapter_failure_is_not_retried() {
        let temp = tempfile::tempdir().expect("temp");
        let mut worker = WorkloadComputeWorker::new("seat15".into(), 1)
            .with_state_root(temp.path().to_path_buf())
            .with_authorizer(Box::new(AllowAuthorizer))
            .with_capacity(test_capacity())
            .with_actuator(Box::new(PermanentActuator));
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("ledger");
        let request = request();
        let raw = serde_json::to_string(&request).expect("wire");
        worker.handle_request(&mut ledger, &raw, request, now_ms());
        let status = ledger.status("op-1").expect("status");
        assert_eq!(status.phase, WorkloadOperationPhase::Failed);
        assert_eq!(status.attempt, 0);
        assert!(!status.retryable);
        assert!(status
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("approved VM image")));
    }

    #[test]
    fn unknown_host_capacity_fails_closed_before_adapter() {
        let temp = tempfile::tempdir().expect("temp");
        let calls = Arc::new(Mutex::new(0));
        let mut worker = WorkloadComputeWorker::new("seat15".into(), 1)
            .with_state_root(temp.path().to_path_buf())
            .with_authorizer(Box::new(AllowAuthorizer))
            .with_capacity(HostCapacity {
                logical_cpus: 0,
                memory_mb: 0,
                allocated_vcpu: 0,
                allocated_memory_mb: 0,
                storage_gb: 0,
                allocated_storage_gb: 0,
            })
            .with_actuator(Box::new(FakeActuator {
                calls: calls.clone(),
            }));
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("ledger");
        let request = request();
        let raw = serde_json::to_string(&request).expect("wire");
        worker.handle_request(&mut ledger, &raw, request, now_ms());
        let status = ledger.status("op-1").expect("status");
        assert_eq!(status.phase, WorkloadOperationPhase::Failed);
        assert_eq!(*calls.lock().expect("calls"), 0);
        assert!(status
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("unknown or invalid")));
    }

    #[test]
    fn start_and_attach_cannot_complete_without_a_real_attachment_lease() {
        let temp = tempfile::tempdir().expect("temp");
        let mut worker = WorkloadComputeWorker::new("seat15".into(), 1)
            .with_state_root(temp.path().to_path_buf())
            .with_authorizer(Box::new(AllowAuthorizer))
            .with_capacity(test_capacity())
            .with_actuator(Box::new(PrematureCompleteActuator));
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("ledger");
        let request = request();
        let raw = serde_json::to_string(&request).expect("wire");
        worker.handle_request(&mut ledger, &raw, request.clone(), now_ms());
        worker.reconcile_inflight(&mut ledger, now_ms());
        let status = ledger.status("op-1").expect("status");
        assert_eq!(status.phase, WorkloadOperationPhase::Failed);
        assert!(status
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("first-frame lease")));
        assert!(status.attachment.is_none());
    }
}
