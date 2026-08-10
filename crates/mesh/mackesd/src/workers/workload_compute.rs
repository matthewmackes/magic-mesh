//! WL-ARCH-010 — the node-local workload operation worker.
//!
//! This is the only worker allowed to consume `action/workload/operation`.
//! It journals and validates a request before calling an injected adapter, then
//! publishes one bounded `state/workloads/<node>` projection.  The production
//! adapter uses only libvirt/virtqemud or Quadlet/systemd; tests use a fake.

#![cfg(feature = "async-services")]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
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
    admit_workload_for_backend, reject_duplicate_json_keys, valid_phase_transition,
    workload_state_topic, HostCapacity, WorkloadAdmission, WorkloadAttachmentLease,
    WorkloadAttachmentProtocol, WorkloadBackend, WorkloadOperationAction,
    WorkloadOperationErrorCode, WorkloadOperationPhase, WorkloadOperationReply,
    WorkloadOperationRequest, WorkloadOperationStatus, WorkloadPowerState, WorkloadReadiness,
    WorkloadRuntimeSignals, WorkloadStateSnapshot, WorkloadStorageCapacity,
    MAX_WORKLOAD_WIRE_BYTES, WORKLOAD_CONTRACT_SCHEMA_VERSION, WORKLOAD_OPERATION_TOPIC,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::{Persist, StoredMessage};
use mde_bus::rpc::reply_topic;
use sha2::{Digest, Sha256};

use super::cloud::{
    claim_nonce, verify_token, HmacTokenSigner, NullSigner, TokenSigner, TokenVerdict,
    DEFAULT_AUTH_ROOT,
};
use super::proc::{output_with_timeout, status_with_timeout, DEFAULT_CMD_TIMEOUT};
use super::{ShutdownToken, Worker};
use crate::display1_broker::{
    display1_socket_path_at, register_display1_listener, Display1AttachmentServer,
    Display1InputPoll, Display1InputState, Display1Peer, DISPLAY1_SOCKET_ROOT,
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
/// The hardened seat-user PipeWire-Pulse bridge consumed by system QEMU.
const WORKLOAD_AUDIO_PORT: u16 = 4713;
const WORKLOAD_AUDIO_CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
/// Keep active input responsive while backing off idle attachment threads so a
/// connected but unused guest does not consume a core on a small seat.
const DISPLAY1_INPUT_IDLE_MIN_SLEEP: Duration = Duration::from_millis(5);
const DISPLAY1_INPUT_IDLE_MAX_SLEEP: Duration = Duration::from_millis(25);
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
const REPLY_OUTBOX_DIR: &str = "reply-outbox";
const MAX_REPLY_OUTBOX_RECORDS: usize = 128;
const MAX_REPLY_OUTBOX_RECORD_BYTES: u64 = 128 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BusIdentity {
    device: u64,
    inode: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReplyOutboxPhase {
    Pending,
    Completed,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ReplyOutboxRecord {
    schema_version: u16,
    message_ulid: String,
    request_id: String,
    phase: ReplyOutboxPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reply: Option<WorkloadOperationReply>,
}

/// Host-local durable barrier between a Workload effect and its required Bus
/// reply. It survives worker/daemon restart, so reply failure never re-enters
/// the actuator. It does not make the backend effect and its post-effect ledger
/// transition crash-atomic; recovery from the journaled `Defining` boundary
/// continues to rely on each supported actuator's idempotent reconciliation.
struct ReplyOutbox {
    root: PathBuf,
}

struct BusActivation {
    identity: BusIdentity,
    tail: Option<String>,
    pending_replies: Vec<StagedOutboxRecord>,
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
                "Workload Bus index changed during transaction",
            ));
        }
        Ok(())
    }
}

struct StagedOutboxRecord {
    record: ReplyOutboxRecord,
    existing_reply_body: Option<String>,
}

struct StagedOperationMessage {
    message: StoredMessage,
    outbox: Option<StagedOutboxRecord>,
}

#[cfg(test)]
#[derive(Default)]
struct WorkloadBusFaults {
    fail_action_reads: AtomicU64,
    fail_reply_writes: AtomicU64,
    fail_state_writes: AtomicU64,
    replace_reply_index_after_write: Mutex<Option<PathBuf>>,
    replace_index_after_open: Mutex<Option<PathBuf>>,
}

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

impl ReplyOutbox {
    fn open(state_root: &Path) -> Result<Self, String> {
        fs::create_dir_all(state_root)
            .map_err(|error| format!("create Workload state root: {error}"))?;
        let root = state_root.join(REPLY_OUTBOX_DIR);
        fs::create_dir_all(&root).map_err(|error| format!("create reply outbox: {error}"))?;
        let metadata = fs::symlink_metadata(&root)
            .map_err(|error| format!("inspect reply outbox: {error}"))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("reply outbox is not a regular directory".into());
        }
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("secure reply outbox: {error}"))?;
        Ok(Self { root })
    }

    fn record_path(&self, message_ulid: &str) -> PathBuf {
        self.root.join(format!("{message_ulid}.json"))
    }

    fn validate(record: &ReplyOutboxRecord) -> Result<(), String> {
        if record.schema_version != WORKLOAD_CONTRACT_SCHEMA_VERSION
            || record.message_ulid.len() != 26
            || !record
                .message_ulid
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
            || record.request_id.is_empty()
            || record.request_id.len() > mackes_mesh_types::workloads::MAX_WORKLOAD_IDENTIFIER_BYTES
            || record.request_id.chars().any(char::is_control)
            || (record.phase == ReplyOutboxPhase::Pending && record.reply.is_some())
            || (record.phase == ReplyOutboxPhase::Completed && record.reply.is_none())
        {
            return Err("reply outbox record failed validation".into());
        }
        Ok(())
    }

    fn store(&self, record: &ReplyOutboxRecord) -> Result<(), String> {
        Self::validate(record)?;
        let destination = self.record_path(&record.message_ulid);
        if !destination.exists() {
            let retained = fs::read_dir(&self.root)
                .map_err(|error| format!("list reply outbox: {error}"))?
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .path()
                        .extension()
                        .is_some_and(|value| value == "json")
                })
                .count();
            if retained >= MAX_REPLY_OUTBOX_RECORDS {
                return Err("reply outbox is at capacity".into());
            }
        }
        let body = serde_json::to_vec(record)
            .map_err(|error| format!("encode reply outbox record: {error}"))?;
        if u64::try_from(body.len()).unwrap_or(u64::MAX) > MAX_REPLY_OUTBOX_RECORD_BYTES {
            return Err("reply outbox record is oversized".into());
        }
        let temporary = self.root.join(format!(
            ".{}.{}.{}.tmp",
            record.message_ulid,
            std::process::id(),
            now_ms()
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| format!("create reply outbox record: {error}"))?;
        if let Err(error) = file.write_all(&body).and_then(|()| file.sync_all()) {
            let _ = fs::remove_file(&temporary);
            return Err(format!("persist reply outbox record: {error}"));
        }
        if let Err(error) = fs::rename(&temporary, &destination) {
            let _ = fs::remove_file(&temporary);
            return Err(format!("commit reply outbox record: {error}"));
        }
        fs::File::open(&self.root)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("sync reply outbox directory: {error}"))?;
        Ok(())
    }

    fn decode_path(&self, path: &Path) -> Result<ReplyOutboxRecord, String> {
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| format!("inspect reply outbox record: {error}"))?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > MAX_REPLY_OUTBOX_RECORD_BYTES
        {
            return Err("reply outbox contains an unsafe record".into());
        }
        let body = fs::read(path).map_err(|error| format!("read reply outbox record: {error}"))?;
        let text = std::str::from_utf8(&body)
            .map_err(|_| "reply outbox record is not UTF-8".to_string())?;
        reject_duplicate_json_keys(text)
            .map_err(|_| "reply outbox record has duplicate keys".to_string())?;
        let record: ReplyOutboxRecord = serde_json::from_slice(&body)
            .map_err(|error| format!("decode reply outbox record: {error}"))?;
        Self::validate(&record)?;
        if self.record_path(&record.message_ulid) != path {
            return Err("reply outbox filename does not match its record".into());
        }
        Ok(record)
    }

    fn load(&self, message_ulid: &str) -> Result<Option<ReplyOutboxRecord>, String> {
        let path = self.record_path(message_ulid);
        if !path.exists() {
            return Ok(None);
        }
        self.decode_path(&path).map(Some)
    }

    fn pending(&self) -> Result<Vec<ReplyOutboxRecord>, String> {
        let mut paths = fs::read_dir(&self.root)
            .map_err(|error| format!("list reply outbox: {error}"))?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|value| value == "json"))
            .collect::<Vec<_>>();
        paths.sort();
        if paths.len() > MAX_REPLY_OUTBOX_RECORDS {
            return Err("reply outbox exceeds its bounded capacity".into());
        }
        paths.iter().map(|path| self.decode_path(path)).collect()
    }

    fn remove(&self, message_ulid: &str) -> Result<(), String> {
        match fs::remove_file(self.record_path(message_ulid)) {
            Ok(()) => fs::File::open(&self.root)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| format!("sync reply outbox cleanup: {error}")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("remove reply outbox record: {error}")),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestartRecoveryStep {
    WaitForStop,
    JournalStarting,
    StartBackend,
    ObserveGuest,
}

fn restart_recovery_step(
    phase: WorkloadOperationPhase,
    running: bool,
) -> Option<RestartRecoveryStep> {
    match (phase, running) {
        (WorkloadOperationPhase::Stopping, true) => Some(RestartRecoveryStep::WaitForStop),
        (WorkloadOperationPhase::Stopping, false) => Some(RestartRecoveryStep::JournalStarting),
        (WorkloadOperationPhase::Starting, false) => Some(RestartRecoveryStep::StartBackend),
        (WorkloadOperationPhase::Starting, true) => Some(RestartRecoveryStep::ObserveGuest),
        _ => None,
    }
}

fn restart_stop_verb(backend: WorkloadBackend, running: bool) -> Option<&'static str> {
    running.then_some(if backend.is_vm() { "shutdown" } else { "stop" })
}

fn recover_restart<F>(
    phase: WorkloadOperationPhase,
    running: bool,
    start_backend: F,
) -> Result<Option<WorkloadActuatorOutcome>, WorkloadActuatorError>
where
    F: FnOnce() -> Result<(), WorkloadActuatorError>,
{
    let Some(step) = restart_recovery_step(phase, running) else {
        return Ok(None);
    };
    let outcome = match step {
        RestartRecoveryStep::WaitForStop => WorkloadActuatorOutcome {
            phase: WorkloadOperationPhase::Stopping,
            power: WorkloadPowerState::Stopping,
            readiness: WorkloadReadiness::Unavailable,
            retryable: true,
            reason: Some("restart is waiting for the backend to stop".into()),
            remediation: None,
            attachment: None,
        },
        RestartRecoveryStep::JournalStarting => WorkloadActuatorOutcome {
            phase: WorkloadOperationPhase::Starting,
            power: WorkloadPowerState::Stopped,
            readiness: WorkloadReadiness::Unavailable,
            retryable: true,
            reason: Some("restart stop completed; the durable journal now authorizes start".into()),
            remediation: None,
            attachment: None,
        },
        RestartRecoveryStep::StartBackend => {
            start_backend()?;
            WorkloadActuatorOutcome {
                phase: WorkloadOperationPhase::WaitingForGuest,
                power: WorkloadPowerState::Starting,
                readiness: WorkloadReadiness::WaitingForGuest,
                retryable: true,
                reason: Some("restart start was issued from its durable phase".into()),
                remediation: None,
                attachment: None,
            }
        }
        RestartRecoveryStep::ObserveGuest => WorkloadActuatorOutcome {
            phase: WorkloadOperationPhase::WaitingForGuest,
            power: WorkloadPowerState::Starting,
            readiness: WorkloadReadiness::WaitingForGuest,
            retryable: true,
            reason: Some("restart start survived recovery; observing guest readiness".into()),
            remediation: None,
            attachment: None,
        },
    };
    Ok(Some(outcome))
}

/// Runtime ownership for one authenticated Display1 lease. The server is
/// created before the VM side effect; the QEMU peer is registered only after
/// libvirt reports a running DBus graphics endpoint. Keeping the peer alive is
/// part of the attachment contract—dropping it would silently unregister the
/// listener while the Workload still reports progress.
struct Display1AttachmentRuntime {
    server: Arc<Display1AttachmentServer>,
    peer: Arc<Mutex<Option<Arc<Display1Peer>>>>,
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
        let server = Arc::clone(&self.server);
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
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|runtime| format!("build Display1 runtime: {runtime}"));
                let result = runtime.as_ref().map_err(Clone::clone).and_then(|runtime| {
                    runtime
                        .block_on(tokio::time::timeout(
                            Duration::from_secs(5),
                            register_display1_listener(&qemu_address, sink),
                        ))
                        .map_err(|_| "QEMU Display1 listener registration timed out".to_string())?
                        .map_err(|attach| format!("register QEMU Display1 listener: {attach}"))
                });
                match result {
                    Ok(display1_peer) => {
                        let display1_peer = Arc::new(display1_peer);
                        if let Ok(mut slot) = peer.lock() {
                            *slot = Some(Arc::clone(&display1_peer));
                            registration.store(DISPLAY1_REGISTRATION_READY, Ordering::Release);
                        } else {
                            registration.store(DISPLAY1_REGISTRATION_FAILED, Ordering::Release);
                        }
                        let runtime = runtime.expect("registration requires a runtime");
                        let mut input = Display1InputState::default();
                        let mut input_epoch = server.input_epoch();
                        let mut pending_lifecycle_release = false;
                        let mut idle_input_polls = 0_u32;
                        while !shutdown.load(Ordering::Acquire) {
                            if display1_peer.qemu.is_closed() {
                                let _ = runtime.block_on(input.release_all(&display1_peer));
                                if let Ok(mut slot) = error.lock() {
                                    *slot = Some(
                                        "QEMU Display1 control connection closed; input authority was revoked"
                                            .into(),
                                    );
                                }
                                registration
                                    .store(DISPLAY1_REGISTRATION_FAILED, Ordering::Release);
                                break;
                            }
                            let current_epoch = server.input_epoch();
                            if current_epoch != input_epoch {
                                input_epoch = current_epoch;
                                pending_lifecycle_release = true;
                            }
                            if pending_lifecycle_release {
                                match runtime.block_on(input.replace_relay(&display1_peer)) {
                                    Ok(()) => pending_lifecycle_release = false,
                                    Err(release_error) => {
                                        // Retain failed held edges and retry. A relay
                                        // may attach while this converges, but no fresh
                                        // input is admitted against inverted state.
                                        tracing::warn!(
                                            error = %release_error,
                                            "Display1 lifecycle release will retry"
                                        );
                                        thread::sleep(Duration::from_millis(5));
                                        continue;
                                    }
                                }
                            }
                            let result = match server.poll_input() {
                                Ok(Display1InputPoll::Input(message)) => {
                                    idle_input_polls = 0;
                                    runtime.block_on(input.apply(&display1_peer, message))
                                }
                                Ok(Display1InputPoll::Disconnected) => {
                                    idle_input_polls = 0;
                                    pending_lifecycle_release = true;
                                    Ok(())
                                }
                                Ok(Display1InputPoll::Idle) => {
                                    idle_input_polls = idle_input_polls.saturating_add(1);
                                    Ok(())
                                }
                                Err(error) => Err(error),
                            };
                            if let Err(reason) = result {
                                let reason = format!("QEMU Display1 input failed closed: {reason}");
                                if let Ok(mut slot) = error.lock() {
                                    *slot = Some(reason);
                                }
                                registration
                                    .store(DISPLAY1_REGISTRATION_FAILED, Ordering::Release);
                                break;
                            }
                            thread::sleep(display1_input_sleep(idle_input_polls));
                        }
                        let _ = runtime.block_on(input.release_all(&display1_peer));
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

/// Return the polling delay for one attachment's current input idleness.
///
/// The first idle polls retain the old 5 ms input latency. Once the relay has
/// stayed idle, the delay rises in small steps and is capped at 25 ms, avoiding
/// a busy loop without making a fresh input feel sluggish.
fn display1_input_sleep(idle_polls: u32) -> Duration {
    let step = u64::from((idle_polls / 8).min(4));
    let min_millis = u64::try_from(DISPLAY1_INPUT_IDLE_MIN_SLEEP.as_millis()).unwrap_or(u64::MAX);
    let max_millis = u64::try_from(DISPLAY1_INPUT_IDLE_MAX_SLEEP.as_millis()).unwrap_or(u64::MAX);
    let millis = min_millis
        .saturating_add(step.saturating_mul(5))
        .min(max_millis);
    Duration::from_millis(millis)
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

    fn observe_not_running(
        &self,
        request: &WorkloadOperationRequest,
        status: &WorkloadOperationStatus,
    ) -> Result<Option<WorkloadActuatorOutcome>, WorkloadActuatorError> {
        match status.phase {
            WorkloadOperationPhase::Stopping => Ok(Some(self.stopped_outcome(request))),
            WorkloadOperationPhase::WaitingForGuest => Err(WorkloadActuatorError::Retryable(
                "the workload is not yet observed running while waiting for guest readiness".into(),
            )),
            _ => Ok(None),
        }
    }

    fn qemu_display1_address(
        request: &WorkloadOperationRequest,
    ) -> Result<String, WorkloadActuatorError> {
        let domain = Self::libvirt_domain(request);
        let mut command = Command::new("virsh");
        command.args([
            "--connect",
            "qemu:///system",
            "domdisplay",
            "--type",
            "dbus",
            &domain,
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
        let domain = Self::libvirt_domain(request);
        let mut command = Command::new("virsh");
        command.args([
            "--connect",
            "qemu:///system",
            "dominfo",
            &domain,
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
        ensure_new_overlay_path(&disk)?;
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
                let _ = fs::remove_file(&disk);
                WorkloadActuatorError::Retryable(format!(
                    "VM overlay creation failed to start: {error}"
                ))
            })?;
        if !image_status.success() {
            let _ = fs::remove_file(&disk);
            return Err(WorkloadActuatorError::Retryable(format!(
                "VM overlay creation exited with {image_status}"
            )));
        }

        let spec = crate::workers::workload_vm::VmDomainSpec {
            name: domain.to_string(),
            vcpus: u32::from(request.resources.vcpu),
            ram_mb: u64::from(request.resources.memory_mb),
            host_threads: std::thread::available_parallelism().map_or(1, |parallelism| {
                u32::try_from(parallelism.get()).unwrap_or(u32::MAX)
            }),
            network: Some("default".to_string()),
        };
        let xml = crate::workers::workload_vm::build_domain_xml(&spec, &disk_string)
            .map_err(|error| {
                let _ = fs::remove_file(&disk);
                WorkloadActuatorError::Permanent(format!("invalid VM resources: {error}"))
            })?;
        let xml_path = std::env::temp_dir().join(format!(
            "mde-workload-{domain}-{}-{}.xml",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let mut xml_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&xml_path)
            .map_err(|error| {
                let _ = fs::remove_file(&disk);
                WorkloadActuatorError::Retryable(format!("create VM definition: {error}"))
            })?;
        if let Err(error) = xml_file
            .write_all(xml.as_bytes())
            .and_then(|()| xml_file.sync_all())
        {
            let _ = fs::remove_file(&xml_path);
            let _ = fs::remove_file(&disk);
            return Err(WorkloadActuatorError::Retryable(format!(
                "write VM definition: {error}"
            )));
        }
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

    /// Source identities are globally stable, but the final component alone
    /// is not unique: two app VMs can share an app name or catalog revision.
    /// Keep the full identity in a bounded deterministic domain name, with a
    /// digest suffix preventing truncation collisions.
    fn libvirt_domain(request: &WorkloadOperationRequest) -> String {
        let identity = request.workload_id.as_str();
        let readable = identity
            .rsplit(':')
            .next()
            .unwrap_or(identity)
            .chars()
            .filter(|value| value.is_ascii_alphanumeric() || matches!(value, '-' | '_' | '.'))
            .take(40)
            .collect::<String>();
        let digest = Sha256::digest(identity.as_bytes());
        let digest = digest[..8]
            .iter()
            .map(|value| format!("{value:02x}"))
            .collect::<String>();
        if readable.is_empty() {
            format!("mde-vm-{digest}")
        } else {
            format!("mde-vm-{readable}-{digest}")
        }
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
        destroy.args(["--connect", "qemu:///system", "destroy", &domain]);
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
            &domain,
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
                if verb == "start" {
                    require_workload_audio_endpoint()?;
                }
                let domain = Self::libvirt_domain(request);
                let mut command = Command::new("virsh");
                command.args([
                    "--connect",
                    "qemu:///system",
                    verb,
                    &domain,
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
                let domain = Self::libvirt_domain(request);
                let mut command = Command::new("virsh");
                command.args([
                    "--connect",
                    "qemu:///system",
                    "domstate",
                    &domain,
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

fn ensure_new_overlay_path(path: &Path) -> Result<(), WorkloadActuatorError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(WorkloadActuatorError::Permanent(
            "managed VM overlay already exists without an admitted domain".into(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(WorkloadActuatorError::Retryable(format!(
            "inspect managed VM overlay path: {error}"
        ))),
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
        require_workload_audio_endpoint().map_err(WorkloadActuatorError::Retryable)?;
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

fn require_workload_audio_endpoint() -> Result<(), String> {
    require_workload_audio_endpoint_at(
        SocketAddr::from(([127, 0, 0, 1], WORKLOAD_AUDIO_PORT)),
        WORKLOAD_AUDIO_CONNECT_TIMEOUT,
    )
}

fn require_workload_audio_endpoint_at(
    endpoint: SocketAddr,
    timeout: Duration,
) -> Result<(), String> {
    TcpStream::connect_timeout(&endpoint, timeout)
        .map(|_| ())
        .map_err(|error| {
            format!(
                "Workload VM audio endpoint {endpoint} is unavailable: {error}; restore mcnf-qemu-pulse-endpoint.service before starting the VM"
            )
        })
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
        if validate_recovered_attachment_lease(request, status, lease, now_ms).is_err() {
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
        validate_native_attachment_route(request)?;
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
            let runtime = match self.ensure_attachment(
                    request,
                    request.expected_generation.saturating_add(1).max(1),
                    now_ms(),
                ) {
                Ok(runtime) => runtime,
                Err(error) if request.backend == WorkloadBackend::QuadletSystemd => {
                    if let Err(cleanup) = Self::remove_container_unit(request) {
                        return Err(WorkloadActuatorError::Retryable(format!(
                            "attachment setup failed ({error}); Quadlet cleanup failed: {cleanup}"
                        )));
                    }
                    return Err(error);
                }
                Err(error) => return Err(error),
            };
            Some(runtime.server.lease().clone())
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
                    if let Err(error) = self.define_vm(request) {
                        // The attachment is provisioned before the VM so the
                        // definition can bind its authenticated endpoint. A
                        // definition failure must not strand that capability
                        // while the operation retries or becomes terminal.
                        self.remove_attachment(request);
                        return Err(error);
                    }
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
                if let Err(error) = Self::run_power_command(request, "start") {
                    if request.action == WorkloadOperationAction::StartAndAttach {
                        // A failed start leaves the definition eligible for a
                        // bounded retry, but it did not establish a guest
                        // endpoint. Do not retain a live Display1 capability
                        // across that failed backend boundary.
                        self.remove_attachment(request);
                    }
                    return Err(error.into());
                }
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
                // Restart is deliberately split into journaled stop and start
                // phases. Replaying Defining may repeat only the idempotent
                // stop request; Starting is persisted before start, so a
                // daemon crash can observe an already-running backend instead
                // of issuing a second restart.
                if let Some(verb) = restart_stop_verb(request.backend, Self::running(request)?) {
                    Self::run_power_command(request, verb)?;
                }
                self.remove_attachment(request);
                WorkloadActuatorOutcome {
                    phase: WorkloadOperationPhase::Stopping,
                    power: WorkloadPowerState::Stopping,
                    readiness: WorkloadReadiness::Unavailable,
                    retryable: true,
                    reason: Some("restart is waiting for the backend to stop".into()),
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
        if request.action == WorkloadOperationAction::Restart {
            if let Some(outcome) = recover_restart(status.phase, running, || {
                Self::run_power_command(request, "start").map_err(WorkloadActuatorError::Retryable)
            })? {
                return Ok(Some(outcome));
            }
        }
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
        } else {
            return self.observe_not_running(request, status);
        };
        Ok(Some(outcome))
    }
}

fn validate_native_attachment_route(
    request: &WorkloadOperationRequest,
) -> Result<(), WorkloadActuatorError> {
    if request.action != WorkloadOperationAction::StartAndAttach {
        return Ok(());
    }
    if request.backend != WorkloadBackend::LibvirtVirtqemud {
        return Err(WorkloadActuatorError::Permanent(
            "StartAndAttach is available only for libvirt VM workloads; headless containers must use Start"
                .into(),
        ));
    }
    if request.preferred_attachment != Some(WorkloadAttachmentProtocol::QemuDisplay1Dmabuf) {
        return Err(WorkloadActuatorError::Permanent(
            "the local Workload actuator supports StartAndAttach only through QEMU Display1 DMA-BUF"
                .into(),
        ));
    }
    Ok(())
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
    bus_root_override: Option<PathBuf>,
    bus_disabled: bool,
    bus_identity: Option<BusIdentity>,
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
    #[cfg(test)]
    bus_faults: Arc<WorkloadBusFaults>,
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
            bus_root_override: None,
            bus_disabled: false,
            bus_identity: None,
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
            #[cfg(test)]
            bus_faults: Arc::new(WorkloadBusFaults::default()),
        }
    }

    fn bus_root(&self) -> Option<PathBuf> {
        if self.bus_disabled {
            return None;
        }
        Some(workload_bus_root_or_system(
            self.bus_root_override
                .clone()
                .or_else(mde_bus::default_data_dir),
        ))
    }

    fn open_bus(&self) -> io::Result<Option<(PathBuf, Persist, BusIdentity)>> {
        let Some(root) = self.bus_root() else {
            return Ok(None);
        };
        let identity_before = match bus_identity(&root) {
            Ok(identity) => identity,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // Persist may legitimately create a late Bus. Discard that
                // initializer connection, then bracket the connection we
                // actually return with identity-before/after proof.
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
            .expect("open replacement fault mutex")
            .take()
        {
            install_replacement_index(&root, &replacement)?;
        }
        let identity_after = bus_identity(&root)?;
        if identity_before != identity_after {
            return Err(io::Error::other(
                "Workload Bus index changed while opening transaction",
            ));
        }
        Ok(Some((root, persist, identity_after)))
    }

    fn stage_activation(
        &self,
        transaction: BusTransaction<'_>,
        outbox: &ReplyOutbox,
    ) -> io::Result<BusActivation> {
        #[cfg(test)]
        if take_fault(&self.bus_faults.fail_action_reads) {
            return Err(io::Error::other(
                "injected Workload activation tail failure",
            ));
        }
        let tail = transaction
            .persist
            .latest_ulid(ACTION_TOPIC)
            .map_err(io_other)?;
        let pending = outbox.pending().map_err(io::Error::other)?;
        let mut pending_replies = Vec::with_capacity(pending.len());
        for record in pending {
            let existing_reply_body = transaction
                .persist
                .read_latest(&reply_topic(&record.message_ulid))
                .map_err(io_other)?
                .and_then(|message| message.body);
            if let Some(reply) = &record.reply {
                let _ = serde_json::to_string(reply).map_err(io_other)?;
            }
            pending_replies.push(StagedOutboxRecord {
                record,
                existing_reply_body,
            });
        }
        transaction.verify_current()?;
        Ok(BusActivation {
            identity: transaction.identity,
            tail,
            pending_replies,
        })
    }

    fn complete_pending_reply(
        &self,
        outbox: &ReplyOutbox,
        ledger: &WorkloadOperationLedger,
        mut record: ReplyOutboxRecord,
    ) -> io::Result<ReplyOutboxRecord> {
        if record.phase == ReplyOutboxPhase::Pending {
            let reply = ledger.status(&record.request_id).cloned().map_or_else(
                || {
                    HandleResult::Rejected(WorkloadOperationErrorCode::JournalUnavailable)
                        .reply(record.request_id.clone())
                },
                |status| HandleResult::Accepted(status).reply(record.request_id.clone()),
            );
            record.phase = ReplyOutboxPhase::Completed;
            record.reply = Some(reply);
            outbox.store(&record).map_err(io::Error::other)?;
        }
        Ok(record)
    }

    fn deliver_outbox_reply(
        &self,
        transaction: BusTransaction<'_>,
        outbox: &ReplyOutbox,
        record: &ReplyOutboxRecord,
        existing_reply_body: Option<&str>,
    ) -> io::Result<()> {
        let reply = record
            .reply
            .as_ref()
            .ok_or_else(|| io::Error::other("completed Workload outbox record has no reply"))?;
        let expected = serde_json::to_string(reply).map_err(io_other)?;
        let already_published = existing_reply_body.is_some_and(|body| body == expected);
        if !already_published {
            self.write_operation_reply(transaction, &record.message_ulid, reply)?;
        }
        // The write above uses an already-open SQLite connection. External
        // replacement can move that connection onto a retired index, so prove
        // the path still names the staged index before deleting the only
        // durable retry record.
        transaction.verify_current()?;
        outbox
            .remove(&record.message_ulid)
            .map_err(io::Error::other)?;
        if let Err(error) = transaction.verify_current() {
            // Replacement can race the unlink after the pre-cleanup identity
            // check. Restore the completed record so the current index still
            // has a durable corrected-forward path.
            outbox.store(record).map_err(io::Error::other)?;
            return Err(error);
        }
        Ok(())
    }

    fn recover_activation_replies(
        &self,
        transaction: BusTransaction<'_>,
        outbox: &ReplyOutbox,
        ledger: &WorkloadOperationLedger,
        records: Vec<StagedOutboxRecord>,
    ) -> io::Result<Vec<ReplyOutboxRecord>> {
        let mut delivered = Vec::new();
        for staged in records {
            let record = self.complete_pending_reply(outbox, ledger, staged.record)?;
            if let Err(error) = self.deliver_outbox_reply(
                transaction,
                outbox,
                &record,
                staged.existing_reply_body.as_deref(),
            ) {
                for prior in &delivered {
                    outbox.store(prior).map_err(io::Error::other)?;
                }
                return Err(error);
            }
            delivered.push(record);
        }
        if let Err(error) = transaction.verify_current() {
            for record in &delivered {
                outbox.store(record).map_err(io::Error::other)?;
            }
            return Err(error);
        }
        Ok(delivered)
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
        self.bus_disabled = root.is_none();
        self.bus_root_override = root;
        self
    }

    #[cfg(test)]
    fn with_bus_faults(mut self, faults: Arc<WorkloadBusFaults>) -> Self {
        self.bus_faults = faults;
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
        // Once accepted, cancellation owns cleanup of its exact journaled
        // target even if the request's client-facing deadline passes. Falling
        // through generic expiry here would cancel the cancellation record
        // itself and strand (or even restart) the target operation.
        if request.action == WorkloadOperationAction::Cancel {
            self.drive_cancel(ledger, request, status, now_ms);
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
        }
        if status.phase == WorkloadOperationPhase::Validating {
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
        // A terminal failure cannot retain an attachment capability. This is
        // especially important during restart recovery: a persisted in-flight
        // StartAndAttach may already carry a Display1 lease when observation
        // fails permanently. Revoke that exact identity before journaling the
        // failure so neither the durable record nor its projection can expose
        // a stale session endpoint.
        self.actuator.revoke_attachment(&status);
        status.attachment = None;
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

    /// Expiration after the defining boundary is a cleanup operation, not a
    /// terminal status shortcut.  The adapter may already own a VM, overlay,
    /// unit, or Display1 runtime, so keep the durable operation in flight and
    /// therefore exclusive until idempotent cancellation proves everything is
    /// stopped.  This also covers daemon restart: the journaled phase is the
    /// authority for deciding whether side effects may exist.
    fn cleanup_expired_operation(
        &self,
        ledger: &mut WorkloadOperationLedger,
        request: &WorkloadOperationRequest,
        mut status: WorkloadOperationStatus,
        now_ms: u64,
    ) {
        if matches!(
            status.phase,
            WorkloadOperationPhase::Queued
                | WorkloadOperationPhase::Validating
                | WorkloadOperationPhase::Admitting
        ) {
            self.fail(
                ledger,
                request,
                status,
                "workload operation deadline expired before adapter side effects",
                "issue a new operation from the current Workload projection",
                false,
                now_ms,
            );
            return;
        }
        if now_ms < status.next_retry_at_ms {
            return;
        }

        // The visible lease must not outlive the deadline even if stopping the
        // backend needs another poll.  Revocation is exact and idempotent.
        self.actuator.revoke_attachment(&status);
        status.attachment = None;

        match self.actuator.cancel(request, &status) {
            Ok(mut outcome)
                if outcome.phase == WorkloadOperationPhase::Cancelled
                    && outcome.power == WorkloadPowerState::Stopped
                    && outcome.readiness == WorkloadReadiness::Unavailable
                    && outcome.attachment.is_none() =>
            {
                outcome.reason = Some(
                    "expired workload operation was cancelled and all adapter resources were cleaned up"
                        .into(),
                );
                outcome.remediation =
                    Some("issue a new operation from the current Workload projection".into());
                self.apply_outcome(ledger, request, status, outcome, now_ms);
            }
            Ok(_) | Err(_) => {
                // Never publish terminal failure while cleanup is unproven.
                // Keeping this generation nonterminal also prevents a second
                // open from creating a duplicate App VM session.
                let attempt = status.attempt.saturating_add(1);
                let exponent = u32::from(attempt.saturating_sub(1).min(5));
                let delay = (1_000_u64.saturating_mul(1_u64 << exponent)).min(MAX_RETRY_BACKOFF_MS);
                status.attempt = attempt;
                status.phase = WorkloadOperationPhase::Stopping;
                status.power = WorkloadPowerState::Stopping;
                status.readiness = WorkloadReadiness::Unavailable;
                status.retryable = true;
                status.next_retry_at_ms = now_ms.saturating_add(delay);
                status.reason =
                    Some("workload deadline expired; adapter cleanup is still in progress".into());
                status.remediation = Some(
                    "wait for the authoritative Workload cleanup to finish before retrying".into(),
                );
                status.signals =
                    WorkloadRuntimeSignals::from_readiness(status.phase, status.readiness);
                if let Err(error) = ledger.advance(&request.request_id, status, now_ms) {
                    tracing::error!(%error, "expired workload cleanup state could not be journaled");
                }
            }
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
                        | WorkloadOperationPhase::Validating
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
        // A nonterminal cancellation is the sole owner of its target's next
        // adapter effect. In particular, a restart target journaled as
        // Stopping must not independently recover into Starting while its
        // cancellation is waiting on durable backoff.
        let cancellation_targets: BTreeSet<_> = pending
            .iter()
            .filter_map(|status| ledger.request(&status.request_id))
            .filter(|request| request.action == WorkloadOperationAction::Cancel)
            .filter_map(|request| request.target_request_id.clone())
            .collect();
        for status in pending {
            let Some(request) = ledger.request(&status.request_id).cloned() else {
                continue;
            };
            if request.action != WorkloadOperationAction::Cancel
                && cancellation_targets.contains(&request.request_id)
            {
                continue;
            }
            if request.action == WorkloadOperationAction::Cancel {
                self.drive_accepted(ledger, request, status, now_ms);
                continue;
            }
            if request.deadline_at_ms <= now_ms {
                self.cleanup_expired_operation(ledger, &request, status, now_ms);
                continue;
            }
            // Backoff is durable state, not merely an admission-time hint.
            // Honor it for post-admission observations as well as for the
            // queued/defining path, otherwise every poll tick can turn one
            // transient adapter error into an unbounded restart storm.
            if now_ms < status.next_retry_at_ms {
                continue;
            }
            if matches!(
                status.phase,
                WorkloadOperationPhase::Queued
                    | WorkloadOperationPhase::Validating
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
            .filter(|status| {
                if !status.phase.is_terminal() {
                    return false;
                }
                if status.attachment.is_some() {
                    return true;
                }
                // A completed StartAndAttach that survived without its lease
                // must still be inspected.  Otherwise a stale journal can
                // publish Ready without an authenticated Display1 capability.
                status.readiness == WorkloadReadiness::Ready
                    && ledger
                        .request(&status.request_id)
                        .is_some_and(|request| {
                            request.action == WorkloadOperationAction::StartAndAttach
                        })
            })
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
            if status.attachment.is_none() && status.readiness == WorkloadReadiness::Ready {
                self.refuse_recovered_attachment(
                    ledger,
                    status,
                    "the recovered StartAndAttach record reported Ready without a journaled Display1 lease and was revoked",
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
            // A restart cannot turn the adapter's missing attachment into a
            // successful StartAndAttach projection.  Recovery is the only
            // authority allowed to recreate the exact lease, so Ready without
            // that confirmation must revoke the stale descriptor and fail
            // closed instead of advertising a usable session.
            if outcome.readiness == WorkloadReadiness::Ready
                && outcome.attachment.is_none()
            {
                tracing::error!(
                    request_id = %request.request_id,
                    "terminal attachment recovery reported Ready without a lease"
                );
                self.refuse_recovered_attachment(
                    ledger,
                    status,
                    "recovered Display1 attachment reported Ready without an authoritative lease and was revoked",
                    now_ms,
                );
                continue;
            }
            if let Some(lease) = outcome.attachment.as_ref() {
                let Some(persisted_lease) = status.attachment.as_ref() else {
                    tracing::error!(
                        request_id = %request.request_id,
                        "terminal attachment recovery lost its journaled lease"
                    );
                    let mut uncommitted = status.clone();
                    uncommitted.attachment = Some(lease.clone());
                    self.actuator.revoke_attachment(&uncommitted);
                    self.refuse_recovered_attachment(
                        ledger,
                        status,
                        "recovered Display1 attachment had no journaled lease authority and was refused",
                        now_ms,
                    );
                    continue;
                };
                let substituted_lease = lease != persisted_lease;
                if substituted_lease
                    || validate_recovered_attachment_lease(&request, &status, lease, now_ms)
                        .is_err()
                {
                    tracing::error!(
                        request_id = %request.request_id,
                        "terminal attachment recovery returned an unauthorized or invalid lease"
                    );
                    // The adapter may already have materialized the returned
                    // endpoint. Revoke that exact uncommitted capability as
                    // well as the journaled lease; otherwise a hostile or
                    // buggy recovery result can leave an untracked Display1
                    // socket alive after the durable projection fails closed.
                    if substituted_lease {
                        let mut uncommitted = status.clone();
                        uncommitted.attachment = Some(lease.clone());
                        self.actuator.revoke_attachment(&uncommitted);
                    }
                    self.refuse_recovered_attachment(
                        ledger,
                        status,
                        "recovered Display1 attachment did not reproduce the exact journaled lease and was refused",
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
        let received_attachment = outcome.attachment.is_some();
        for phase in steps {
            status.phase = phase;
            status.signals = WorkloadRuntimeSignals::from_readiness(phase, status.readiness);
            if phase == outcome.phase {
                // The adapter's lease belongs to the outcome commit point, not
                // to synthetic intermediate phases. Persisting it early could
                // expose an attachment before its admitting phase survived a
                // crash. If any transition rejects the outcome, revoke the
                // still-uncommitted adapter capability before returning.
                if received_attachment || outcome.phase == WorkloadOperationPhase::Cancelled {
                    status.attachment = outcome.attachment.clone();
                }
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
                    if received_attachment {
                        let mut uncommitted = status.clone();
                        uncommitted.attachment = outcome.attachment.clone();
                        self.actuator.revoke_attachment(&uncommitted);
                    }
                    tracing::error!(%error, "workload observation result could not be journaled");
                    return;
                }
            }
        }
    }

    fn publish(
        &mut self,
        transaction: Option<BusTransaction<'_>>,
        ledger: &WorkloadOperationLedger,
        now_ms: u64,
    ) -> io::Result<()> {
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
            return Ok(());
        }
        let snapshot = WorkloadStateSnapshot {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            node: self.node_id.clone(),
            observed_at_ms: now_ms,
            workloads: statuses.clone(),
        };
        if let Err(error) = snapshot.validate(now_ms) {
            tracing::error!(%error, "workload state projection refused");
            return Err(io::Error::other(error.to_string()));
        }
        if let Some(transaction) = transaction {
            #[cfg(test)]
            if take_fault(&self.bus_faults.fail_state_writes) {
                return Err(io::Error::other(
                    "injected Workload state publication failure",
                ));
            }
            let body = serde_json::to_string(&snapshot).map_err(io_other)?;
            transaction
                .persist
                .write(
                    &workload_state_topic(&self.node_id),
                    Priority::Default,
                    None,
                    Some(&body),
                )
                .map_err(io_other)?;
            transaction.verify_current()?;
            self.last_projection = Some(statuses);
        }
        Ok(())
    }

    fn write_operation_reply(
        &self,
        transaction: BusTransaction<'_>,
        message_ulid: &str,
        reply: &WorkloadOperationReply,
    ) -> io::Result<()> {
        #[cfg(test)]
        if take_fault(&self.bus_faults.fail_reply_writes) {
            return Err(io::Error::other(
                "injected Workload operation reply failure",
            ));
        }
        let body = serde_json::to_string(reply).map_err(io_other)?;
        transaction
            .persist
            .write(
                &reply_topic(message_ulid),
                Priority::Default,
                None,
                Some(&body),
            )
            .map_err(io_other)?;
        #[cfg(test)]
        if let Some(replacement) = self
            .bus_faults
            .replace_reply_index_after_write
            .lock()
            .expect("replacement fault mutex")
            .take()
        {
            install_replacement_index(transaction.root, &replacement)?;
        }
        Ok(())
    }

    fn stage_operation_messages(
        &self,
        persist: &Persist,
        outbox: &ReplyOutbox,
    ) -> io::Result<Vec<StagedOperationMessage>> {
        #[cfg(test)]
        if take_fault(&self.bus_faults.fail_action_reads) {
            return Err(io::Error::other("injected Workload action read failure"));
        }
        let messages = persist
            .list_since_limit(
                ACTION_TOPIC,
                self.cursor.as_deref(),
                MAX_OPERATION_MESSAGES_PER_TICK,
            )
            .map_err(io_other)?;
        let mut staged = Vec::with_capacity(messages.len());
        for message in messages {
            let outbox_record = outbox.load(&message.ulid).map_err(io::Error::other)?;
            let outbox = if let Some(record) = outbox_record {
                let existing_reply_body = persist
                    .read_latest(&reply_topic(&record.message_ulid))
                    .map_err(io_other)?
                    .and_then(|reply| reply.body);
                Some(StagedOutboxRecord {
                    record,
                    existing_reply_body,
                })
            } else {
                None
            };
            staged.push(StagedOperationMessage { message, outbox });
        }
        Ok(staged)
    }

    fn completed_outbox_record(
        message_ulid: String,
        reply: WorkloadOperationReply,
    ) -> ReplyOutboxRecord {
        ReplyOutboxRecord {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            message_ulid,
            request_id: reply.request_id.clone(),
            phase: ReplyOutboxPhase::Completed,
            reply: Some(reply),
        }
    }

    fn process_staged_messages(
        &mut self,
        ledger: &mut WorkloadOperationLedger,
        transaction: BusTransaction<'_>,
        outbox: &ReplyOutbox,
        staged: Vec<StagedOperationMessage>,
        now: u64,
    ) -> io::Result<()> {
        for staged_message in staged {
            let message = staged_message.message;
            if let Some(staged_outbox) = staged_message.outbox {
                let record = self.complete_pending_reply(outbox, ledger, staged_outbox.record)?;
                self.deliver_outbox_reply(
                    transaction,
                    outbox,
                    &record,
                    staged_outbox.existing_reply_body.as_deref(),
                )?;
                self.cursor = Some(message.ulid);
                continue;
            }

            let body = message.body.as_deref().unwrap_or("");
            if body.len() > MAX_WORKLOAD_WIRE_BYTES {
                tracing::warn!("oversized workload operation refused");
                let record = Self::completed_outbox_record(
                    message.ulid.clone(),
                    HandleResult::Rejected(WorkloadOperationErrorCode::PayloadTooLarge)
                        .reply(safe_request_id(body)),
                );
                outbox.store(&record).map_err(io::Error::other)?;
                self.deliver_outbox_reply(transaction, outbox, &record, None)?;
                self.cursor = Some(message.ulid);
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
                    let record = Self::completed_outbox_record(
                        message.ulid.clone(),
                        HandleResult::Rejected(code).reply(safe_request_id(body)),
                    );
                    outbox.store(&record).map_err(io::Error::other)?;
                    self.deliver_outbox_reply(transaction, outbox, &record, None)?;
                    self.cursor = Some(message.ulid);
                    continue;
                }
            };
            if request.target_node != self.node_id {
                transaction.verify_current()?;
                self.cursor = Some(message.ulid);
                continue;
            }
            let request_id = request.request_id.clone();
            let pending = ReplyOutboxRecord {
                schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
                message_ulid: message.ulid.clone(),
                request_id: request_id.clone(),
                phase: ReplyOutboxPhase::Pending,
                reply: None,
            };
            outbox.store(&pending).map_err(io::Error::other)?;
            let result = self.handle_request(ledger, body, request, now);
            let completed =
                Self::completed_outbox_record(message.ulid.clone(), result.reply(request_id));
            outbox.store(&completed).map_err(io::Error::other)?;
            self.deliver_outbox_reply(transaction, outbox, &completed, None)?;
            self.cursor = Some(message.ulid);
        }
        Ok(())
    }

    fn tick_once_result(
        &mut self,
        ledger: &mut WorkloadOperationLedger,
        transaction: Option<BusTransaction<'_>>,
        outbox: &ReplyOutbox,
    ) -> io::Result<()> {
        let staged = transaction
            .map(|transaction| self.stage_operation_messages(transaction.persist, outbox))
            .transpose()?
            .unwrap_or_default();
        if let Some(transaction) = transaction {
            transaction.verify_current()?;
        }
        self.drain_migration_commands();
        let now = now_ms();
        if let Some(transaction) = transaction {
            self.process_staged_messages(ledger, transaction, outbox, staged, now)?;
        }
        self.reconcile_inflight(ledger, now);
        self.reconcile_recovered_attachments(ledger, now);
        self.actuator.reap_expired(now);
        self.publish(transaction, ledger, now)
    }

    fn tick_once(
        &mut self,
        ledger: &mut WorkloadOperationLedger,
        persist: Option<(&Persist, &Path)>,
    ) {
        let result = ReplyOutbox::open(&self.state_root)
            .map_err(io::Error::other)
            .and_then(|outbox| {
                let transaction = persist
                    .map(|(persist, root)| {
                        bus_identity(root).map(|identity| BusTransaction {
                            persist,
                            root,
                            identity,
                        })
                    })
                    .transpose()?;
                self.tick_once_result(ledger, transaction, &outbox)
            });
        if let Err(error) = result {
            tracing::warn!(target: "mackesd::workload_compute", %error, "Workload transaction deferred");
        }
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
        let outbox = ReplyOutbox::open(&self.state_root)
            .map_err(|error| anyhow::anyhow!("open workload reply outbox: {error}"))?;
        loop {
            match self.open_bus() {
                Err(error) => {
                    tracing::warn!(target: "mackesd::workload_compute", %error, "Workload Bus unavailable; worker will retry");
                }
                Ok(None) => {
                    if let Err(error) = self.tick_once_result(&mut ledger, None, &outbox) {
                        tracing::warn!(target: "mackesd::workload_compute", %error, "disabled-Bus Workload reconciliation deferred");
                    }
                }
                Ok(Some((root, persist, identity))) => {
                    let transaction = BusTransaction {
                        persist: &persist,
                        root: &root,
                        identity,
                    };
                    let activated = if self.bus_identity == Some(identity) {
                        true
                    } else {
                        match self.stage_activation(transaction, &outbox) {
                            Ok(activation) => {
                                match self.recover_activation_replies(
                                    transaction,
                                    &outbox,
                                    &ledger,
                                    activation.pending_replies,
                                ) {
                                    Ok(delivered) => match transaction.verify_current() {
                                        Ok(()) => {
                                            self.cursor = activation.tail;
                                            self.bus_identity = Some(activation.identity);
                                            self.last_projection = None;
                                            true
                                        }
                                        Err(error) => {
                                            for record in &delivered {
                                                if let Err(restore_error) = outbox.store(record) {
                                                    tracing::error!(target: "mackesd::workload_compute", %restore_error, "Workload reply outbox restore failed after activation replacement");
                                                }
                                            }
                                            tracing::warn!(target: "mackesd::workload_compute", %error, "Workload Bus changed before activation commit");
                                            false
                                        }
                                    },
                                    Err(error) => {
                                        tracing::warn!(target: "mackesd::workload_compute", %error, "Workload reply recovery deferred activation");
                                        false
                                    }
                                }
                            }
                            Err(error) => {
                                tracing::warn!(target: "mackesd::workload_compute", %error, "Workload Bus activation deferred");
                                false
                            }
                        }
                    };
                    if activated {
                        if let Err(error) =
                            self.tick_once_result(&mut ledger, Some(transaction), &outbox)
                        {
                            tracing::warn!(target: "mackesd::workload_compute", %error, "Workload Bus transaction deferred");
                        }
                    }
                }
            }
            tokio::select! {
                () = shutdown.wait() => break,
                () = tokio::time::sleep(self.poll_interval) => {}
            }
        }
        Ok(())
    }
}

fn workload_bus_root_or_system(resolved: Option<PathBuf>) -> PathBuf {
    resolved.unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
}

fn bus_identity(root: &Path) -> io::Result<BusIdentity> {
    let metadata = fs::metadata(root.join("index.sqlite"))?;
    if !metadata.is_file() {
        return Err(io::Error::other("Workload Bus index is not a regular file"));
    }
    use std::os::unix::fs::MetadataExt;
    Ok(BusIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(test)]
fn install_replacement_index(root: &Path, replacement: &Path) -> io::Result<()> {
    // A test replacement models an external installer publishing one complete
    // SQLite generation. Retired WAL state must not leak into that generation.
    for sidecar in ["index.sqlite-wal", "index.sqlite-shm"] {
        match fs::remove_file(root.join(sidecar)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    fs::rename(replacement, root.join("index.sqlite"))
}

fn io_other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

#[cfg(test)]
fn take_fault(counter: &AtomicU64) -> bool {
    counter
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
            remaining.checked_sub(1)
        })
        .is_ok()
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
            "stop another workload or choose the Small profile",
        ),
        Some(mackes_mesh_types::workloads::AdmissionDenial::MemoryReserve) => (
            "workload would consume the reserved host memory",
            "stop another workload or choose the Small profile",
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
    if valid_phase_transition(from, to) {
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

fn validate_recovered_attachment_lease(
    request: &WorkloadOperationRequest,
    status: &WorkloadOperationStatus,
    lease: &WorkloadAttachmentLease,
    now_ms: u64,
) -> Result<(), &'static str> {
    if lease.workload_id != request.workload_id || lease.workload_id != status.workload_id {
        return Err("lease workload identity does not match the recovered Workload");
    }
    if lease.generation != status.generation {
        return Err("lease generation does not match the recovered Workload generation");
    }
    if lease.protocol != WorkloadAttachmentProtocol::QemuDisplay1Dmabuf {
        return Err("lease protocol is not the authenticated Display1 protocol");
    }
    if lease.expires_at_ms > request.deadline_at_ms {
        return Err("lease outlives the originating Workload operation deadline");
    }
    lease
        .validate(now_ms)
        .map_err(|_| "recovered Workload lease is expired or malformed")
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

    #[test]
    fn restart_recovery_journals_start_before_the_only_start_effect() {
        assert_eq!(
            restart_stop_verb(WorkloadBackend::LibvirtVirtqemud, true),
            Some("shutdown")
        );
        assert_eq!(
            restart_stop_verb(WorkloadBackend::QuadletSystemd, true),
            Some("stop")
        );
        assert_eq!(
            restart_stop_verb(WorkloadBackend::LibvirtVirtqemud, false),
            None
        );

        assert_eq!(
            restart_recovery_step(WorkloadOperationPhase::Stopping, true),
            Some(RestartRecoveryStep::WaitForStop)
        );
        assert_eq!(
            restart_recovery_step(WorkloadOperationPhase::Stopping, false),
            Some(RestartRecoveryStep::JournalStarting)
        );
        assert_eq!(
            restart_recovery_step(WorkloadOperationPhase::Starting, false),
            Some(RestartRecoveryStep::StartBackend)
        );
        assert_eq!(
            restart_recovery_step(WorkloadOperationPhase::Starting, true),
            Some(RestartRecoveryStep::ObserveGuest)
        );
        assert!(valid_phase_transition(
            WorkloadOperationPhase::Defining,
            WorkloadOperationPhase::Stopping
        ));
        assert!(valid_phase_transition(
            WorkloadOperationPhase::Stopping,
            WorkloadOperationPhase::Starting
        ));
        assert!(valid_phase_transition(
            WorkloadOperationPhase::Starting,
            WorkloadOperationPhase::WaitingForGuest
        ));
        assert_eq!(
            phase_steps(
                WorkloadOperationPhase::Defining,
                WorkloadOperationPhase::Stopping
            ),
            Some(vec![WorkloadOperationPhase::Stopping])
        );
        assert_eq!(
            phase_steps(
                WorkloadOperationPhase::Stopping,
                WorkloadOperationPhase::Starting
            ),
            Some(vec![WorkloadOperationPhase::Starting])
        );
    }

    #[test]
    fn capacity_refusal_recommends_the_smaller_profile() {
        for denial in [
            mackes_mesh_types::workloads::AdmissionDenial::CpuReserve,
            mackes_mesh_types::workloads::AdmissionDenial::MemoryReserve,
        ] {
            let (_, remediation) = admission_message(WorkloadAdmission {
                admitted: false,
                denial: Some(denial),
                available_vcpu: 0,
                available_memory_mb: 0,
                available_storage_gb: 0,
            });
            assert_eq!(
                remediation,
                "stop another workload or choose the Small profile"
            );
        }
    }

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

    struct RestartRecoveryActuator {
        running: Arc<AtomicBool>,
        start_calls: Arc<AtomicU64>,
    }

    impl WorkloadActuator for RestartRecoveryActuator {
        fn apply(
            &self,
            _: &WorkloadOperationRequest,
        ) -> Result<WorkloadActuatorOutcome, WorkloadActuatorError> {
            unreachable!("reopened Starting state must be observed, not applied")
        }

        fn cancel(
            &self,
            _: &WorkloadOperationRequest,
            _: &WorkloadOperationStatus,
        ) -> Result<WorkloadActuatorOutcome, WorkloadActuatorError> {
            unreachable!("restart recovery test does not cancel")
        }

        fn observe(
            &self,
            request: &WorkloadOperationRequest,
            status: &WorkloadOperationStatus,
        ) -> Result<Option<WorkloadActuatorOutcome>, WorkloadActuatorError> {
            assert_eq!(request.action, WorkloadOperationAction::Restart);
            let running = self.running.load(Ordering::Acquire);
            recover_restart(status.phase, running, || {
                self.start_calls.fetch_add(1, Ordering::AcqRel);
                self.running.store(true, Ordering::Release);
                Ok(())
            })
        }
    }

    fn seed_restart_starting(root: &Path, now: u64) {
        let mut request = request();
        request.action = WorkloadOperationAction::Restart;
        request.preferred_attachment = None;
        let mut ledger = WorkloadOperationLedger::open(root).expect("restart ledger");
        let mut status = ledger.accept(request, now).expect("accept restart");
        for phase in [
            WorkloadOperationPhase::Validating,
            WorkloadOperationPhase::Admitting,
            WorkloadOperationPhase::Defining,
            WorkloadOperationPhase::Stopping,
            WorkloadOperationPhase::Starting,
        ] {
            status.phase = phase;
            if phase == WorkloadOperationPhase::Stopping {
                status.power = WorkloadPowerState::Stopping;
                status.readiness = WorkloadReadiness::Unavailable;
            } else if phase == WorkloadOperationPhase::Starting {
                status.power = WorkloadPowerState::Stopped;
                status.readiness = WorkloadReadiness::Unavailable;
            }
            status.signals = WorkloadRuntimeSignals::from_readiness(phase, status.readiness);
            status = ledger
                .advance("op-1", status, now)
                .expect("journal restart phase");
        }
    }

    #[test]
    fn reopened_starting_restart_counts_the_only_start_effect_and_advances_journal() {
        for (already_running, expected_starts) in [(true, 0), (false, 1)] {
            let state = tempfile::tempdir().expect("restart state");
            let now = now_ms();
            seed_restart_starting(state.path(), now);

            // Reopen the real journal to model a daemon crash after Starting
            // was durable but before its WaitingForGuest outcome was flushed.
            let mut ledger = WorkloadOperationLedger::open(state.path()).expect("reopen ledger");
            assert_eq!(
                ledger.status("op-1").expect("starting status").phase,
                WorkloadOperationPhase::Starting
            );
            let running = Arc::new(AtomicBool::new(already_running));
            let start_calls = Arc::new(AtomicU64::new(0));
            let mut worker = WorkloadComputeWorker::new("seat15".into(), 1).with_actuator(
                Box::new(RestartRecoveryActuator {
                    running: Arc::clone(&running),
                    start_calls: Arc::clone(&start_calls),
                }),
            );

            worker.reconcile_inflight(&mut ledger, now.saturating_add(1));
            assert_eq!(start_calls.load(Ordering::Acquire), expected_starts);
            assert_eq!(
                ledger.status("op-1").expect("advanced status").phase,
                WorkloadOperationPhase::WaitingForGuest
            );
            drop(ledger);

            let mut reopened =
                WorkloadOperationLedger::open(state.path()).expect("reopen advanced ledger");
            assert_eq!(
                reopened.status("op-1").expect("durable status").phase,
                WorkloadOperationPhase::WaitingForGuest
            );
            worker.reconcile_inflight(&mut reopened, now.saturating_add(2));
            assert_eq!(
                start_calls.load(Ordering::Acquire),
                expected_starts,
                "replaying the advanced journal repeated the start effect"
            );
        }
    }

    struct RestartCancellationActuator {
        cancel_actions: Arc<Mutex<Vec<WorkloadOperationAction>>>,
        observe_calls: Arc<AtomicU64>,
    }

    impl WorkloadActuator for RestartCancellationActuator {
        fn apply(
            &self,
            _: &WorkloadOperationRequest,
        ) -> Result<WorkloadActuatorOutcome, WorkloadActuatorError> {
            unreachable!("restart cancellation recovery must not apply")
        }

        fn cancel(
            &self,
            request: &WorkloadOperationRequest,
            _: &WorkloadOperationStatus,
        ) -> Result<WorkloadActuatorOutcome, WorkloadActuatorError> {
            let mut actions = self.cancel_actions.lock().expect("cancel actions");
            actions.push(request.action);
            let complete = actions.len() > 1;
            Ok(WorkloadActuatorOutcome {
                phase: if complete {
                    WorkloadOperationPhase::Cancelled
                } else {
                    WorkloadOperationPhase::Stopping
                },
                power: if complete {
                    WorkloadPowerState::Stopped
                } else {
                    WorkloadPowerState::Stopping
                },
                readiness: WorkloadReadiness::Unavailable,
                retryable: !complete,
                reason: Some(
                    if complete {
                        "restart target cleanup completed"
                    } else {
                        "restart target is still stopping"
                    }
                    .into(),
                ),
                remediation: None,
                attachment: None,
            })
        }

        fn observe(
            &self,
            _: &WorkloadOperationRequest,
            _: &WorkloadOperationStatus,
        ) -> Result<Option<WorkloadActuatorOutcome>, WorkloadActuatorError> {
            self.observe_calls.fetch_add(1, Ordering::AcqRel);
            Ok(None)
        }
    }

    #[test]
    fn reopened_expired_cancel_owns_restart_target_until_cleanup_completes() {
        let state = tempfile::tempdir().expect("cancellation state");
        let started_at = now_ms();
        seed_restart_starting(state.path(), started_at);
        let cancel_actions = Arc::new(Mutex::new(Vec::new()));
        let observe_calls = Arc::new(AtomicU64::new(0));
        let actuator = || RestartCancellationActuator {
            cancel_actions: Arc::clone(&cancel_actions),
            observe_calls: Arc::clone(&observe_calls),
        };

        let retry_at = {
            let mut ledger = WorkloadOperationLedger::open(state.path()).expect("restart ledger");
            let target = ledger.request("op-1").expect("restart request").clone();
            let target_status = ledger.status("op-1").expect("restart status").clone();
            let mut cancel = target.clone();
            cancel.request_id = "cancel-restart".into();
            cancel.action = WorkloadOperationAction::Cancel;
            cancel.expected_generation = target_status.generation;
            cancel.target_request_id = Some(target.request_id.clone());
            cancel.deadline_at_ms = started_at.saturating_add(500);
            cancel.preferred_attachment = None;
            let raw = serde_json::to_string(&cancel).expect("cancel wire");
            let mut worker = WorkloadComputeWorker::new("seat15".into(), 1)
                .with_authorizer(Box::new(AllowAuthorizer))
                .with_capacity(test_capacity())
                .with_actuator(Box::new(actuator()));

            worker.handle_request(&mut ledger, &raw, cancel, started_at);
            assert_eq!(
                ledger.status("op-1").expect("stopping target").phase,
                WorkloadOperationPhase::Stopping
            );
            ledger
                .status("cancel-restart")
                .expect("waiting cancellation")
                .next_retry_at_ms
        };
        assert!(retry_at > started_at.saturating_add(500));

        // Reopen after the cancellation deadline and at its durable retry.
        // The cancellation must keep ownership: the restart target cannot be
        // observed into Starting and cleanup must receive the target request,
        // never the cancellation request itself.
        let mut ledger = WorkloadOperationLedger::open(state.path()).expect("reopened ledger");
        let mut worker = WorkloadComputeWorker::new("seat15".into(), 1)
            .with_authorizer(Box::new(AllowAuthorizer))
            .with_capacity(test_capacity())
            .with_actuator(Box::new(actuator()));
        worker.reconcile_inflight(&mut ledger, retry_at);

        assert_eq!(observe_calls.load(Ordering::Acquire), 0);
        assert_eq!(
            cancel_actions.lock().expect("cancel actions").as_slice(),
            &[
                WorkloadOperationAction::Restart,
                WorkloadOperationAction::Restart
            ]
        );
        assert_eq!(
            ledger.status("op-1").expect("cancelled target").phase,
            WorkloadOperationPhase::Cancelled
        );
        assert_eq!(
            ledger
                .status("cancel-restart")
                .expect("completed cancellation")
                .phase,
            WorkloadOperationPhase::Completed
        );
    }

    struct TimeoutCleanupActuator {
        apply_calls: Arc<Mutex<u32>>,
        cleanup_calls: Arc<Mutex<u32>>,
        revoked: Arc<Mutex<Vec<(String, u64)>>>,
        lease: WorkloadAttachmentLease,
    }

    impl WorkloadActuator for TimeoutCleanupActuator {
        fn apply(
            &self,
            _: &WorkloadOperationRequest,
        ) -> Result<WorkloadActuatorOutcome, WorkloadActuatorError> {
            *self.apply_calls.lock().expect("apply calls") += 1;
            Ok(WorkloadActuatorOutcome {
                phase: WorkloadOperationPhase::WaitingForGuest,
                power: WorkloadPowerState::Starting,
                readiness: WorkloadReadiness::WaitingForGuest,
                retryable: true,
                reason: None,
                remediation: None,
                attachment: Some(self.lease.clone()),
            })
        }

        fn cancel(
            &self,
            _: &WorkloadOperationRequest,
            _: &WorkloadOperationStatus,
        ) -> Result<WorkloadActuatorOutcome, WorkloadActuatorError> {
            let mut calls = self.cleanup_calls.lock().expect("cleanup calls");
            *calls += 1;
            if *calls == 1 {
                return Ok(WorkloadActuatorOutcome {
                    phase: WorkloadOperationPhase::Stopping,
                    power: WorkloadPowerState::Stopping,
                    readiness: WorkloadReadiness::Unavailable,
                    retryable: true,
                    reason: Some("hostile backend is still stopping".into()),
                    remediation: None,
                    attachment: None,
                });
            }
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
            panic!("an expired operation must clean up instead of being observed")
        }

        fn revoke_attachment(&self, status: &WorkloadOperationStatus) {
            if let Some(lease) = status.attachment.as_ref() {
                self.revoked
                    .lock()
                    .expect("revocations")
                    .push((lease.lease_id.clone(), lease.generation));
            }
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

    struct PermanentObserveAttachmentActuator {
        revoked: Arc<Mutex<Vec<(String, u64)>>>,
    }

    impl WorkloadActuator for PermanentObserveAttachmentActuator {
        fn apply(
            &self,
            _: &WorkloadOperationRequest,
        ) -> Result<WorkloadActuatorOutcome, WorkloadActuatorError> {
            unreachable!("recovered in-flight operation must be observed, not applied")
        }

        fn cancel(
            &self,
            _: &WorkloadOperationRequest,
            _: &WorkloadOperationStatus,
        ) -> Result<WorkloadActuatorOutcome, WorkloadActuatorError> {
            unreachable!("unexpired recovered operation must not enter cleanup")
        }

        fn observe(
            &self,
            _: &WorkloadOperationRequest,
            _: &WorkloadOperationStatus,
        ) -> Result<Option<WorkloadActuatorOutcome>, WorkloadActuatorError> {
            Err(WorkloadActuatorError::Permanent(
                "recovered Display1 endpoint has hostile peer credentials".into(),
            ))
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

    struct HostileAttachmentOutcomeActuator {
        calls: Arc<Mutex<u32>>,
        revoked: Arc<Mutex<Vec<(String, u64)>>>,
        lease: WorkloadAttachmentLease,
    }

    impl WorkloadActuator for HostileAttachmentOutcomeActuator {
        fn apply(
            &self,
            _: &WorkloadOperationRequest,
        ) -> Result<WorkloadActuatorOutcome, WorkloadActuatorError> {
            *self.calls.lock().expect("apply calls") += 1;
            Ok(WorkloadActuatorOutcome {
                phase: WorkloadOperationPhase::WaitingForFirstFrame,
                power: WorkloadPowerState::Running,
                readiness: WorkloadReadiness::PreparingDisplay,
                retryable: true,
                reason: None,
                remediation: None,
                attachment: Some(self.lease.clone()),
            })
        }

        fn cancel(
            &self,
            _: &WorkloadOperationRequest,
            _: &WorkloadOperationStatus,
        ) -> Result<WorkloadActuatorOutcome, WorkloadActuatorError> {
            unreachable!("hostile result is rejected before cleanup")
        }

        fn observe(
            &self,
            _: &WorkloadOperationRequest,
            _: &WorkloadOperationStatus,
        ) -> Result<Option<WorkloadActuatorOutcome>, WorkloadActuatorError> {
            unreachable!("hostile result test does not reconcile again")
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
            preferred_attachment: Some(WorkloadAttachmentProtocol::QemuDisplay1Dmabuf),
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
    fn invalid_start_and_attach_routes_fail_before_runtime_effects() {
        let mut request = request();
        request.backend = WorkloadBackend::QuadletSystemd;
        request.preferred_attachment = Some(WorkloadAttachmentProtocol::QemuDisplay1Dmabuf);
        assert!(matches!(
            validate_native_attachment_route(&request),
            Err(WorkloadActuatorError::Permanent(reason))
                if reason.contains("headless containers must use Start")
        ));

        request.backend = WorkloadBackend::LibvirtVirtqemud;
        request.preferred_attachment = Some(WorkloadAttachmentProtocol::Vnc);
        assert!(matches!(
            validate_native_attachment_route(&request),
            Err(WorkloadActuatorError::Permanent(reason))
                if reason.contains("only through QEMU Display1 DMA-BUF")
        ));

        request.preferred_attachment = Some(WorkloadAttachmentProtocol::QemuDisplay1Dmabuf);
        assert!(validate_native_attachment_route(&request).is_ok());
    }

    #[test]
    fn stable_source_identity_maps_to_the_libvirt_domain_name() {
        let mut request = request();
        request.workload_id = WorkloadId::new("vm:seat15:browser").expect("id");
        let domain = SystemWorkloadActuator::libvirt_domain(&request);
        assert!(domain.starts_with("mde-vm-browser-"));
        assert_eq!(domain.len(), "mde-vm-browser-".len() + 16);
        assert_eq!(
            SystemWorkloadActuator::vm_overlay_path(&request),
            Path::new("/var/lib/mde-vms").join(format!("{domain}.qcow2"))
        );
    }

    #[test]
    fn hostile_workload_id_suffixes_cannot_alias_a_vm_domain_or_overlay() {
        let mut first = request();
        first.workload_id = WorkloadId::new("app-vm:seat15:writer:org.example.Writer:catalog-7")
            .expect("first id");
        let mut second = first.clone();
        second.workload_id = WorkloadId::new("app-vm:seat15:reader:org.example.Reader:catalog-7")
            .expect("second id");

        let first_domain = SystemWorkloadActuator::libvirt_domain(&first);
        let second_domain = SystemWorkloadActuator::libvirt_domain(&second);
        assert_ne!(
            first_domain, second_domain,
            "full Workload identity must survive the libvirt naming boundary"
        );
        assert_ne!(
            SystemWorkloadActuator::vm_overlay_path(&first),
            SystemWorkloadActuator::vm_overlay_path(&second),
            "colliding final components must not share a managed overlay"
        );
        assert!(first_domain.len() <= 63);
        assert!(second_domain.len() <= 63);
    }

    #[test]
    fn startup_not_running_observation_cannot_complete_as_stopped() {
        let temp = tempfile::tempdir().expect("temp");
        let actuator = SystemWorkloadActuator::new(temp.path().to_path_buf());
        let request = request();
        let mut status = queued_status(&request);
        status.phase = WorkloadOperationPhase::WaitingForGuest;
        status.power = WorkloadPowerState::Starting;
        status.readiness = WorkloadReadiness::WaitingForGuest;

        let error = actuator
            .observe_not_running(&request, &status)
            .expect_err("startup absence must remain a retryable readiness failure");
        assert!(matches!(error, WorkloadActuatorError::Retryable(_)));

        status.phase = WorkloadOperationPhase::Stopping;
        status.power = WorkloadPowerState::Stopping;
        status.readiness = WorkloadReadiness::Unavailable;
        let stopped = actuator
            .observe_not_running(&request, &status)
            .expect("stop observation")
            .expect("terminal stop outcome");
        assert_eq!(stopped.phase, WorkloadOperationPhase::Completed);
        assert_eq!(stopped.power, WorkloadPowerState::Stopped);
        assert_eq!(stopped.readiness, WorkloadReadiness::Unavailable);
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
    fn vm_start_requires_a_reachable_local_audio_endpoint() {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .expect("audio fixture listener");
        let live = listener.local_addr().expect("fixture address");
        require_workload_audio_endpoint_at(live, Duration::from_millis(100))
            .expect("reachable endpoint");

        let closed = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .expect("closed fixture listener");
        let closed_address = closed.local_addr().expect("closed fixture address");
        drop(closed);
        let error = require_workload_audio_endpoint_at(
            closed_address,
            Duration::from_millis(100),
        )
        .expect_err("closed endpoint must prevent a silent VM start");
        assert!(error.contains("restore mcnf-qemu-pulse-endpoint.service"));
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
    fn recovered_attachment_lease_cannot_outlive_its_operation_deadline() {
        let now = now_ms();
        let mut request = request();
        request.deadline_at_ms = now.saturating_add(10_000);
        let status = queued_status(&request);
        let mut lease = SystemWorkloadActuator::attachment_lease(&request, 1, now);
        lease.expires_at_ms = request.deadline_at_ms.saturating_add(1);

        let error = validate_recovered_attachment_lease(&request, &status, &lease, now)
            .expect_err("recovery must reject a lease beyond the request authority window");
        assert!(error.contains("outlives"));
    }

    #[test]
    fn existing_vm_overlay_is_not_overwritten_or_deleted() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("existing.qcow2");
        fs::write(&path, b"retained overlay").expect("seed overlay");

        let error = ensure_new_overlay_path(&path)
            .expect_err("an orphan overlay must require explicit recovery");
        assert!(matches!(error, WorkloadActuatorError::Permanent(_)));
        assert_eq!(fs::read(&path).expect("retained overlay"), b"retained overlay");
    }

    #[test]
    fn display1_input_poll_sleep_backs_off_only_when_idle() {
        assert_eq!(display1_input_sleep(0), Duration::from_millis(5));
        assert_eq!(display1_input_sleep(7), Duration::from_millis(5));
        assert_eq!(display1_input_sleep(8), Duration::from_millis(10));
        assert_eq!(display1_input_sleep(31), Duration::from_millis(20));
        assert_eq!(display1_input_sleep(u32::MAX), Duration::from_millis(25));
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
            .is_some_and(|reason| reason.contains("exact journaled lease")));
        assert!(refused
            .remediation
            .as_deref()
            .is_some_and(|remediation| remediation.contains("current generation")));
    }

    #[test]
    fn recovered_attachment_cannot_substitute_a_new_lease_in_the_same_generation() {
        let temp = tempfile::tempdir().expect("temp");
        let now = now_ms();
        let request = request();
        let lease = SystemWorkloadActuator::attachment_lease(&request, 1, now);
        let mut substituted = lease.clone();
        substituted.lease_id = format!("{}-substituted", substituted.lease_id);
        let revoked = Arc::new(Mutex::new(Vec::new()));
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("ledger");
        seed_completed_attachment(&mut ledger, request.clone(), lease.clone(), now);
        let mut worker = WorkloadComputeWorker::new("seat15".into(), 1).with_actuator(Box::new(
            RecoveryActuator {
                calls: Arc::new(Mutex::new(0)),
                revoked: Arc::clone(&revoked),
                outcome: WorkloadActuatorOutcome {
                    phase: WorkloadOperationPhase::Completed,
                    power: WorkloadPowerState::Running,
                    readiness: WorkloadReadiness::Ready,
                    retryable: false,
                    reason: None,
                    remediation: None,
                    attachment: Some(substituted.clone()),
                },
            },
        ));

        worker.reconcile_recovered_attachments(&mut ledger, now);

        assert_eq!(
            revoked.lock().expect("revoked attachments").as_slice(),
            &[
                (substituted.lease_id, substituted.generation),
                (lease.lease_id, lease.generation),
            ]
        );
        let refused = ledger.status(&request.request_id).expect("refused status");
        assert!(refused.attachment.is_none());
        assert_eq!(refused.readiness, WorkloadReadiness::Unavailable);
        assert!(refused.reason.as_deref().is_some_and(|reason| {
            reason.contains("did not reproduce the exact journaled lease")
        }));
    }

    #[test]
    fn recovered_ready_without_authoritative_lease_is_refused_and_unpublished() {
        let temp = tempfile::tempdir().expect("temp");
        let now = now_ms();
        let request = request();
        let lease = SystemWorkloadActuator::attachment_lease(&request, 1, now);
        let revoked = Arc::new(Mutex::new(Vec::new()));
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("ledger");
        seed_completed_attachment(&mut ledger, request.clone(), lease.clone(), now);
        let mut worker = WorkloadComputeWorker::new("seat15".into(), 1).with_actuator(Box::new(
            RecoveryActuator {
                calls: Arc::new(Mutex::new(0)),
                revoked: Arc::clone(&revoked),
                outcome: WorkloadActuatorOutcome {
                    phase: WorkloadOperationPhase::Completed,
                    power: WorkloadPowerState::Running,
                    readiness: WorkloadReadiness::Ready,
                    retryable: false,
                    reason: None,
                    remediation: None,
                    attachment: None,
                },
            },
        ));

        worker.reconcile_recovered_attachments(&mut ledger, now);

        assert_eq!(
            *revoked.lock().expect("revoked attachments"),
            vec![(lease.lease_id, lease.generation)]
        );
        let refused = ledger.status(&request.request_id).expect("refused status");
        assert!(refused.attachment.is_none());
        assert_eq!(refused.readiness, WorkloadReadiness::Unavailable);
        assert!(refused
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("without an authoritative lease")));
    }

    #[test]
    fn recovered_ready_without_journaled_lease_is_refused_and_unpublished() {
        let temp = tempfile::tempdir().expect("temp");
        let now = now_ms();
        let request = request();
        let lease = SystemWorkloadActuator::attachment_lease(&request, 1, now);
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("ledger");
        seed_completed_attachment(&mut ledger, request.clone(), lease, now);
        let mut status = ledger
            .status(&request.request_id)
            .expect("completed status")
            .clone();
        status.attachment = None;
        status.readiness = WorkloadReadiness::Ready;
        status.signals = WorkloadRuntimeSignals::from_readiness(
            WorkloadOperationPhase::Completed,
            WorkloadReadiness::Ready,
        );
        ledger
            .advance(&request.request_id, status, now)
            .expect("remove stale lease from recovered journal");
        let calls = Arc::new(Mutex::new(0));
        let mut worker = WorkloadComputeWorker::new("seat15".into(), 1).with_actuator(Box::new(
            RecoveryActuator {
                calls: Arc::clone(&calls),
                revoked: Arc::new(Mutex::new(Vec::new())),
                outcome: WorkloadActuatorOutcome {
                    phase: WorkloadOperationPhase::Completed,
                    power: WorkloadPowerState::Running,
                    readiness: WorkloadReadiness::Ready,
                    retryable: false,
                    reason: None,
                    remediation: None,
                    attachment: None,
                },
            },
        ));

        worker.reconcile_recovered_attachments(&mut ledger, now);

        assert_eq!(*calls.lock().expect("recovery calls"), 0);
        let refused = ledger.status(&request.request_id).expect("refused status");
        assert!(refused.attachment.is_none());
        assert_eq!(refused.readiness, WorkloadReadiness::Unavailable);
        assert!(refused.reason.as_deref().is_some_and(|reason| {
            reason.contains("without a journaled Display1 lease")
        }));
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
    fn rejected_attachment_outcome_revokes_uncommitted_lease_without_replay_effect() {
        let temp = tempfile::tempdir().expect("temp");
        let started_at = now_ms();
        let mut request = request();
        request.deadline_at_ms = started_at.saturating_add(20_000);
        let hostile_lease = SystemWorkloadActuator::attachment_lease(&request, 2, started_at);
        let calls = Arc::new(Mutex::new(0));
        let revoked = Arc::new(Mutex::new(Vec::new()));
        let mut worker = WorkloadComputeWorker::new("seat15".into(), 1)
            .with_state_root(temp.path().to_path_buf())
            .with_authorizer(Box::new(AllowAuthorizer))
            .with_capacity(test_capacity())
            .with_actuator(Box::new(HostileAttachmentOutcomeActuator {
                calls: Arc::clone(&calls),
                revoked: Arc::clone(&revoked),
                lease: hostile_lease.clone(),
            }));
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("ledger");
        let raw = serde_json::to_string(&request).expect("wire");

        worker.handle_request(&mut ledger, &raw, request.clone(), started_at);

        let durable = ledger.status(&request.request_id).expect("durable status");
        assert_eq!(durable.phase, WorkloadOperationPhase::PreparingDisplay);
        assert!(durable.attachment.is_none());
        assert_eq!(*calls.lock().expect("apply calls"), 1);
        assert_eq!(
            revoked.lock().expect("revoked attachments").as_slice(),
            &[(hostile_lease.lease_id.clone(), hostile_lease.generation)]
        );

        // The retained Bus delivery is a read-only replay. It neither applies
        // the operation again nor recreates the rejected adapter capability.
        worker.handle_request(&mut ledger, &raw, request, started_at);
        assert_eq!(*calls.lock().expect("apply calls"), 1);
        assert_eq!(revoked.lock().expect("revoked attachments").len(), 1);
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

    async fn wait_for_bus_row(root: &Path, topic: &str) -> StoredMessage {
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
        .expect("timed out waiting for Workload Bus row")
    }

    fn bus_transaction<'a>(persist: &'a Persist, root: &'a Path) -> BusTransaction<'a> {
        BusTransaction {
            persist,
            root,
            identity: bus_identity(root).expect("Bus identity"),
        }
    }

    #[tokio::test]
    async fn worker_recovers_late_and_replaced_bus_without_replaying_retained_actions() {
        let temp = tempfile::tempdir().expect("temp");
        let bus_root = temp.path().join("bus");
        fs::write(&bus_root, b"Bus unavailable").expect("blocking Bus path");
        let staged_root = temp.path().join("staged-bus");
        let staged = Persist::open(staged_root.clone()).expect("staged Bus");
        let mut retained = request();
        retained.request_id = "retained-initial".into();
        let retained_action = staged
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&retained).expect("retained wire")),
            )
            .expect("retained action");
        drop(staged);

        let calls = Arc::new(Mutex::new(0));
        let mut worker = WorkloadComputeWorker::new("seat15".into(), 1)
            .with_bus_root(Some(bus_root.clone()))
            .with_state_root(temp.path().join("state"))
            .with_poll_interval(Duration::from_millis(10))
            .with_authorizer(Box::new(AllowAuthorizer))
            .with_capacity(test_capacity())
            .with_actuator(Box::new(FakeActuator {
                calls: Arc::clone(&calls),
            }));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            !task.is_finished(),
            "missing Bus terminated Workload worker"
        );

        fs::remove_file(&bus_root).expect("remove blocker");
        fs::rename(&staged_root, &bus_root).expect("install Bus");
        wait_for_bus_row(&bus_root, &workload_state_topic("seat15")).await;
        let bus = Persist::open(bus_root.clone()).expect("active Bus");
        assert!(bus
            .list_since(&reply_topic(&retained_action.ulid), None)
            .expect("retained reply query")
            .is_empty());
        assert_eq!(*calls.lock().expect("calls"), 0);

        let first = request();
        let first_action = bus
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&first).expect("first wire")),
            )
            .expect("first forward action");
        wait_for_bus_row(&bus_root, &reply_topic(&first_action.ulid)).await;
        assert_eq!(*calls.lock().expect("calls"), 1);
        drop(bus);

        let replacement_root = temp.path().join("replacement-bus");
        let replacement = Persist::open(replacement_root.clone()).expect("replacement Bus");
        let mut retained_replacement = request();
        retained_replacement.request_id = "retained-replacement".into();
        retained_replacement.workload_id =
            WorkloadId::new("retained-seat15").expect("replacement workload id");
        let retained_replacement_action = replacement
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(
                    &serde_json::to_string(&retained_replacement)
                        .expect("replacement retained wire"),
                ),
            )
            .expect("replacement retained action");
        drop(replacement);
        fs::rename(
            replacement_root.join("index.sqlite"),
            bus_root.join("index.sqlite"),
        )
        .expect("replace Bus index");
        wait_for_bus_row(&bus_root, &workload_state_topic("seat15")).await;
        let replacement = Persist::open(bus_root.clone()).expect("reopened replacement Bus");
        assert!(replacement
            .list_since(&reply_topic(&retained_replacement_action.ulid), None)
            .expect("replacement retained reply query")
            .is_empty());
        assert_eq!(*calls.lock().expect("calls"), 1);

        let mut second = request();
        second.request_id = "forward-replacement".into();
        second.workload_id = WorkloadId::new("second-seat15").expect("second workload id");
        let second_action = replacement
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&second).expect("second wire")),
            )
            .expect("replacement forward action");
        wait_for_bus_row(&bus_root, &reply_topic(&second_action.ulid)).await;
        assert_eq!(*calls.lock().expect("calls"), 2);

        shutdown_tx.send(true).expect("shutdown");
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("Workload worker shutdown timeout")
            .expect("Workload worker joins")
            .expect("Workload worker shutdown succeeds");
        assert_eq!(
            workload_bus_root_or_system(None),
            PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)
        );
        assert!(WorkloadComputeWorker::new("seat15".into(), 1)
            .with_bus_root(None)
            .bus_root()
            .is_none());
    }

    #[test]
    fn atomic_activation_and_durable_reply_recovery_never_repeat_the_effect() {
        let temp = tempfile::tempdir().expect("temp");
        let bus_root = temp.path().join("bus");
        let persist = Persist::open(bus_root.clone()).expect("bus");
        let retained = persist
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(r#"{"request_id":"retained","unknown":true}"#),
            )
            .expect("retained action");
        let state_root = temp.path().join("state");
        let calls = Arc::new(Mutex::new(0));
        let faults = Arc::new(WorkloadBusFaults::default());
        faults.fail_action_reads.store(1, Ordering::SeqCst);
        let mut worker = WorkloadComputeWorker::new("seat15".into(), 1)
            .with_state_root(state_root.clone())
            .with_authorizer(Box::new(AllowAuthorizer))
            .with_capacity(test_capacity())
            .with_actuator(Box::new(FakeActuator {
                calls: Arc::clone(&calls),
            }))
            .with_bus_faults(Arc::clone(&faults));
        let mut ledger = WorkloadOperationLedger::open(&state_root).expect("ledger");
        let outbox = ReplyOutbox::open(&state_root).expect("outbox");
        let transaction = bus_transaction(&persist, &bus_root);

        assert!(worker.stage_activation(transaction, &outbox).is_err());
        assert!(worker.cursor.is_none());
        assert!(worker.bus_identity.is_none());
        let activation = worker
            .stage_activation(transaction, &outbox)
            .expect("atomic activation retry");
        worker
            .recover_activation_replies(transaction, &outbox, &ledger, activation.pending_replies)
            .expect("activation reply recovery");
        worker.cursor = activation.tail;
        worker.bus_identity = Some(activation.identity);
        assert_eq!(worker.cursor.as_deref(), Some(retained.ulid.as_str()));
        assert!(persist
            .list_since(&reply_topic(&retained.ulid), None)
            .expect("retained reply query")
            .is_empty());

        let forward = request();
        let forward_action = persist
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&forward).expect("forward wire")),
            )
            .expect("forward action");
        faults.fail_action_reads.store(1, Ordering::SeqCst);
        assert!(worker
            .tick_once_result(&mut ledger, Some(transaction), &outbox)
            .is_err());
        assert_eq!(*calls.lock().expect("calls"), 0);
        assert_eq!(worker.cursor.as_deref(), Some(retained.ulid.as_str()));

        faults.fail_reply_writes.store(1, Ordering::SeqCst);
        assert!(worker
            .tick_once_result(&mut ledger, Some(transaction), &outbox)
            .is_err());
        assert_eq!(*calls.lock().expect("calls"), 1);
        assert_eq!(worker.cursor.as_deref(), Some(retained.ulid.as_str()));
        assert!(persist
            .list_since(&reply_topic(&forward_action.ulid), None)
            .expect("failed reply query")
            .is_empty());
        drop(ledger);

        let restarted_faults = Arc::new(WorkloadBusFaults::default());
        let mut restarted = WorkloadComputeWorker::new("seat15".into(), 1)
            .with_state_root(state_root.clone())
            .with_authorizer(Box::new(AllowAuthorizer))
            .with_capacity(test_capacity())
            .with_actuator(Box::new(FakeActuator {
                calls: Arc::clone(&calls),
            }))
            .with_bus_faults(Arc::clone(&restarted_faults));
        let mut ledger = WorkloadOperationLedger::open(&state_root).expect("reopened ledger");
        let activation = restarted
            .stage_activation(transaction, &outbox)
            .expect("restart activation");
        restarted
            .recover_activation_replies(transaction, &outbox, &ledger, activation.pending_replies)
            .expect("durable reply recovery");
        restarted.cursor = activation.tail;
        restarted.bus_identity = Some(activation.identity);
        assert_eq!(*calls.lock().expect("calls"), 1);
        assert_eq!(
            persist
                .list_since(&reply_topic(&forward_action.ulid), None)
                .expect("recovered reply")
                .len(),
            1
        );

        let mut second = request();
        second.request_id = "state-retry".into();
        second.workload_id = WorkloadId::new("state-retry-seat15").expect("workload id");
        let second_action = persist
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&second).expect("second wire")),
            )
            .expect("second action");
        restarted_faults
            .fail_state_writes
            .store(1, Ordering::SeqCst);
        assert!(restarted
            .tick_once_result(&mut ledger, Some(transaction), &outbox)
            .is_err());
        assert_eq!(*calls.lock().expect("calls"), 2);
        assert_eq!(
            restarted.cursor.as_deref(),
            Some(second_action.ulid.as_str())
        );
        assert!(restarted.last_projection.is_none());
        restarted
            .tick_once_result(&mut ledger, Some(transaction), &outbox)
            .expect("corrected-forward state publication");
        assert_eq!(*calls.lock().expect("calls"), 2);
        assert!(restarted.last_projection.is_some());
    }

    #[test]
    fn replacement_during_reply_keeps_outbox_and_recovers_into_current_index() {
        let temp = tempfile::tempdir().expect("temp");
        let bus_root = temp.path().join("bus");
        let retired_connection = Persist::open(bus_root.clone()).expect("initial Bus");
        let state_root = temp.path().join("state");
        let calls = Arc::new(Mutex::new(0));
        let faults = Arc::new(WorkloadBusFaults::default());
        let mut worker = WorkloadComputeWorker::new("seat15".into(), 1)
            .with_state_root(state_root.clone())
            .with_authorizer(Box::new(AllowAuthorizer))
            .with_capacity(test_capacity())
            .with_actuator(Box::new(FakeActuator {
                calls: Arc::clone(&calls),
            }))
            .with_bus_faults(Arc::clone(&faults));
        let mut ledger = WorkloadOperationLedger::open(&state_root).expect("ledger");
        let outbox = ReplyOutbox::open(&state_root).expect("outbox");
        let request = request();
        let action = retired_connection
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&request).expect("request wire")),
            )
            .expect("action");
        let retired_transaction = bus_transaction(&retired_connection, &bus_root);

        let replacement_root = temp.path().join("replacement-bus");
        drop(Persist::open(replacement_root.clone()).expect("replacement Bus"));
        *faults
            .replace_reply_index_after_write
            .lock()
            .expect("replacement fault mutex") = Some(replacement_root.join("index.sqlite"));

        let error = worker
            .tick_once_result(&mut ledger, Some(retired_transaction), &outbox)
            .expect_err("replacement must invalidate reply transaction");
        assert!(error.to_string().contains("index changed"));
        assert_eq!(*calls.lock().expect("calls"), 1);
        assert!(worker.cursor.is_none());
        assert!(outbox.load(&action.ulid).expect("outbox read").is_some());
        assert_eq!(
            retired_connection
                .list_since(&reply_topic(&action.ulid), None)
                .expect("retired reply")
                .len(),
            1,
            "the stale connection received the reply before replacement"
        );
        let current = Persist::open(bus_root.clone()).expect("current Bus");
        assert!(current
            .list_since(&reply_topic(&action.ulid), None)
            .expect("current reply before recovery")
            .is_empty());

        let current_transaction = bus_transaction(&current, &bus_root);
        let activation = worker
            .stage_activation(current_transaction, &outbox)
            .expect("replacement activation stages durable reply");
        worker
            .recover_activation_replies(
                current_transaction,
                &outbox,
                &ledger,
                activation.pending_replies,
            )
            .expect("reply recovers into current index");
        current_transaction
            .verify_current()
            .expect("current index before activation commit");
        worker.cursor = activation.tail;
        worker.bus_identity = Some(activation.identity);

        assert_eq!(*calls.lock().expect("calls"), 1);
        assert!(outbox.load(&action.ulid).expect("outbox cleanup").is_none());
        assert_eq!(
            current
                .list_since(&reply_topic(&action.ulid), None)
                .expect("current recovered reply")
                .len(),
            1
        );
    }

    #[test]
    fn replacement_during_open_is_rejected_and_reopens_current_index() {
        const GENERATION_TOPIC: &str = "test/workload/open-generation";

        let temp = tempfile::tempdir().expect("temp");
        let bus_root = temp.path().join("bus");
        let retired = Persist::open(bus_root.clone()).expect("retired Bus");
        retired
            .write(GENERATION_TOPIC, Priority::Default, None, Some("retired"))
            .expect("retired generation marker");
        drop(retired);

        let replacement_root = temp.path().join("replacement-bus");
        let replacement = Persist::open(replacement_root.clone()).expect("replacement Bus");
        replacement
            .write(GENERATION_TOPIC, Priority::Default, None, Some("current"))
            .expect("current generation marker");
        drop(replacement);

        let faults = Arc::new(WorkloadBusFaults::default());
        *faults
            .replace_index_after_open
            .lock()
            .expect("open replacement fault mutex") = Some(replacement_root.join("index.sqlite"));
        let worker = WorkloadComputeWorker::new("seat15".into(), 1)
            .with_bus_root(Some(bus_root.clone()))
            .with_bus_faults(faults);

        let error = match worker.open_bus() {
            Err(error) => error,
            Ok(_) => panic!("an open spanning two Bus generations must be rejected"),
        };
        assert!(error.to_string().contains("changed while opening"));

        let (opened_root, current, identity) = worker
            .open_bus()
            .expect("current generation opens")
            .expect("Bus enabled");
        assert_eq!(opened_root, bus_root);
        assert_eq!(
            identity,
            bus_identity(&opened_root).expect("current identity")
        );
        assert_eq!(
            current
                .read_latest(GENERATION_TOPIC)
                .expect("current generation read")
                .and_then(|message| message.body)
                .as_deref(),
            Some("current")
        );
    }

    #[test]
    fn operation_requests_receive_typed_correlated_replies() {
        let temp = tempfile::tempdir().expect("temp");
        let bus_root = temp.path().join("bus");
        let persist = Persist::open(bus_root.clone()).expect("bus");
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

        worker.tick_once(&mut ledger, Some((&persist, &bus_root)));

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
        worker.tick_once(&mut ledger, Some((&persist, &bus_root)));
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
        let persist = Persist::open(bus_root.clone()).expect("bus");
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

        worker.tick_once(&mut ledger, Some((&persist, &bus_root)));
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

        worker.tick_once(&mut ledger, Some((&persist, &bus_root)));
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
    fn validating_replay_after_restart_resumes_admission_and_drives_one_side_effect() {
        let temp = tempfile::tempdir().expect("temp");
        let calls = Arc::new(Mutex::new(0));
        let request = request();
        {
            let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("ledger");
            let mut status = ledger
                .accept(request.clone(), now_ms())
                .expect("queue request");
            status.phase = WorkloadOperationPhase::Validating;
            ledger
                .advance(&request.request_id, status, now_ms())
                .expect("persist validating crash boundary");
        }

        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("reopened ledger");
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
    fn permanent_observation_failure_after_restart_revokes_persisted_attachment() {
        let temp = tempfile::tempdir().expect("temp");
        let started_at = now_ms();
        let mut request = request();
        request.deadline_at_ms = started_at + 20_000;
        let lease = SystemWorkloadActuator::attachment_lease(&request, 1, started_at);
        {
            let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("ledger");
            let mut status = ledger
                .accept(request.clone(), started_at)
                .expect("queue request");
            for phase in [
                WorkloadOperationPhase::Validating,
                WorkloadOperationPhase::Admitting,
                WorkloadOperationPhase::Defining,
                WorkloadOperationPhase::Starting,
                WorkloadOperationPhase::WaitingForGuest,
                WorkloadOperationPhase::WaitingForService,
                WorkloadOperationPhase::PreparingDisplay,
                WorkloadOperationPhase::WaitingForFirstFrame,
            ] {
                status.phase = phase;
                if phase == WorkloadOperationPhase::WaitingForFirstFrame {
                    status.power = WorkloadPowerState::Running;
                    status.readiness = WorkloadReadiness::PreparingDisplay;
                    status.attachment = Some(lease.clone());
                    status.signals =
                        WorkloadRuntimeSignals::from_readiness(phase, status.readiness);
                }
                status = ledger
                    .advance(&request.request_id, status, started_at)
                    .expect("persist hostile restart boundary");
            }
        }

        let revoked = Arc::new(Mutex::new(Vec::new()));
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("reopened ledger");
        let mut worker = WorkloadComputeWorker::new("seat15".into(), 1).with_actuator(Box::new(
            PermanentObserveAttachmentActuator {
                revoked: Arc::clone(&revoked),
            },
        ));

        worker.reconcile_inflight(&mut ledger, started_at);

        assert_eq!(
            revoked.lock().expect("revoked attachments").as_slice(),
            &[(lease.lease_id, lease.generation)]
        );
        let failed = ledger.status(&request.request_id).expect("failed status");
        assert_eq!(failed.phase, WorkloadOperationPhase::Failed);
        assert_eq!(failed.readiness, WorkloadReadiness::Failed);
        assert!(failed.attachment.is_none());
        assert!(!failed.retryable);
        assert!(failed
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("hostile peer credentials")));
    }

    #[test]
    fn expired_app_vm_open_revokes_lease_and_blocks_duplicates_until_cleanup() {
        let temp = tempfile::tempdir().expect("temp");
        let started_at = now_ms();
        let mut request = request();
        request.workload_id =
            WorkloadId::new("app-vm:seat15:org.example.Editor").expect("App VM workload id");
        request.deadline_at_ms = started_at + 1_000;
        let lease = SystemWorkloadActuator::attachment_lease(&request, 1, started_at);
        let apply_calls = Arc::new(Mutex::new(0));
        let cleanup_calls = Arc::new(Mutex::new(0));
        let revoked = Arc::new(Mutex::new(Vec::new()));
        let mut worker = WorkloadComputeWorker::new("seat15".into(), 1)
            .with_state_root(temp.path().to_path_buf())
            .with_authorizer(Box::new(AllowAuthorizer))
            .with_capacity(test_capacity())
            .with_actuator(Box::new(TimeoutCleanupActuator {
                apply_calls: Arc::clone(&apply_calls),
                cleanup_calls: Arc::clone(&cleanup_calls),
                revoked: Arc::clone(&revoked),
                lease: lease.clone(),
            }));
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("ledger");
        let raw = serde_json::to_string(&request).expect("wire");

        worker.handle_request(&mut ledger, &raw, request.clone(), started_at);
        assert_eq!(*apply_calls.lock().expect("apply calls"), 1);
        assert_eq!(
            ledger
                .status(&request.request_id)
                .and_then(|status| status.attachment.as_ref()),
            Some(&lease)
        );

        // The first cleanup attempt reports that the backend is still
        // stopping.  The lease is already gone, but the operation remains
        // nonterminal so another request cannot open a duplicate session.
        worker.reconcile_inflight(&mut ledger, request.deadline_at_ms);
        let pending = ledger.status(&request.request_id).expect("pending cleanup");
        assert!(!pending.phase.is_terminal());
        assert_eq!(pending.phase, WorkloadOperationPhase::Stopping);
        assert_eq!(pending.power, WorkloadPowerState::Stopping);
        assert!(pending.attachment.is_none());
        assert_eq!(pending.generation, 1);
        assert_eq!(
            revoked.lock().expect("revocations").as_slice(),
            &[(lease.lease_id.clone(), lease.generation)]
        );
        let retry_at = pending.next_retry_at_ms;

        let mut duplicate = request.clone();
        duplicate.request_id = "op-duplicate".into();
        duplicate.deadline_at_ms = request.deadline_at_ms + 20_000;
        let duplicate_raw = serde_json::to_string(&duplicate).expect("duplicate wire");
        assert!(matches!(
            worker.handle_request(
                &mut ledger,
                &duplicate_raw,
                duplicate.clone(),
                request.deadline_at_ms
            ),
            HandleResult::Rejected(WorkloadOperationErrorCode::StaleGeneration)
        ));
        assert!(ledger.request(&duplicate.request_id).is_none());
        assert_eq!(*apply_calls.lock().expect("apply calls"), 1);

        worker.reconcile_inflight(&mut ledger, retry_at);
        let cleaned = ledger.status(&request.request_id).expect("cleaned status");
        assert_eq!(cleaned.phase, WorkloadOperationPhase::Cancelled);
        assert_eq!(cleaned.power, WorkloadPowerState::Stopped);
        assert_eq!(cleaned.readiness, WorkloadReadiness::Unavailable);
        assert!(cleaned.attachment.is_none());
        assert_eq!(cleaned.generation, 1);
        assert_eq!(*cleanup_calls.lock().expect("cleanup calls"), 2);

        // A same-id Bus replay is read-only after cleanup: no second apply,
        // cancellation, lease, or desired generation is produced.
        worker.handle_request(&mut ledger, &raw, request.clone(), retry_at);
        assert_eq!(*apply_calls.lock().expect("apply calls"), 1);
        assert_eq!(*cleanup_calls.lock().expect("cleanup calls"), 2);
        assert_eq!(
            ledger
                .status(&request.request_id)
                .expect("replayed status")
                .generation,
            1
        );
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
