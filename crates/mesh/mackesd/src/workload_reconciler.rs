//! WL-ARCH-010 — durable operation journal for the sole workload reconciler.
//!
//! The journal is intentionally independent of libvirt, Quadlet, QEMU, and
//! the Bus.  It is the boundary that makes a later actuator safe: accept the
//! request and persist `Queued` before any side effect, then persist every
//! phase transition.  A daemon restart can therefore replay the last phase and
//! return the same result for the same idempotency key.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use mackes_mesh_types::workloads::{
    reject_duplicate_json_keys, valid_phase_transition, WorkloadContractError,
    WorkloadOperationAction, WorkloadOperationPhase, WorkloadOperationRequest,
    WorkloadOperationStatus, WORKLOAD_CONTRACT_SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Stable filename inside the daemon's node-local state directory.
pub const WORKLOAD_LEDGER_FILENAME: &str = "workload-operations.json";
/// Maximum number of durable operation records retained by the node journal.
///
/// Active operations are never evicted.  Once a newer generation exists for a
/// workload, older terminal records may be evicted in deterministic order;
/// idempotent replay is therefore guaranteed for retained records only.
pub const MAX_WORKLOAD_OPERATION_RECORDS: usize = 1024;
/// Maximum serialized journal size admitted during restart replay.
const MAX_WORKLOAD_LEDGER_BYTES: u64 = 8 * 1024 * 1024;

/// Errors returned while opening or advancing the durable workload journal.
#[derive(Debug, Error)]
pub enum WorkloadLedgerError {
    /// The state directory or atomic replacement could not be completed.
    #[error("workload journal I/O failed: {0}")]
    Io(#[from] io::Error),
    /// The existing journal was not valid JSON or violated its closed shape.
    #[error("workload journal is malformed")]
    Malformed,
    /// A request or status failed the shared wire contract.
    #[error("workload contract rejected: {0}")]
    Contract(#[from] WorkloadContractError),
    /// An idempotency key was reused for a different request.
    #[error("workload request id already names a different operation: {0}")]
    Conflict(String),
    /// A request names a generation that is older than, or collides with, a
    /// durable operation already accepted for the same workload.
    #[error("stale or colliding workload generation: {0}")]
    StaleGeneration(String),
    /// A newer operation for the same workload is still being reconciled.
    #[error("workload already has an operation in flight: {0}")]
    Busy(String),
    /// The journal is full of active operations or the only terminal records
    /// are still required as the latest generation for their workloads.
    #[error("workload operation journal capacity exhausted")]
    Capacity,
    /// A phase update named an operation that is not in the journal.
    #[error("unknown workload request: {0}")]
    UnknownRequest(String),
    /// The proposed phase does not follow the durable phase.
    #[error("invalid workload phase transition")]
    InvalidTransition,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LedgerRecord {
    request: WorkloadOperationRequest,
    status: WorkloadOperationStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LedgerFile {
    schema_version: u16,
    operations: Vec<LedgerRecord>,
}

/// A node-local, single-writer workload operation journal.
///
/// The reconciler owns one instance.  Callers must use [`Self::accept`] before
/// invoking an actuator and [`Self::advance`] after the actuator's outcome;
/// both methods replace the file atomically and flush it before returning.
#[derive(Debug)]
pub struct WorkloadOperationLedger {
    path: PathBuf,
    operations: BTreeMap<String, LedgerRecord>,
}

impl WorkloadOperationLedger {
    /// Open or create the journal below `state_root`.
    pub fn open(state_root: impl AsRef<Path>) -> Result<Self, WorkloadLedgerError> {
        fs::create_dir_all(state_root.as_ref())?;
        let path = state_root.as_ref().join(WORKLOAD_LEDGER_FILENAME);
        let Some(mut file) = open_restart_ledger(&path)? else {
            return Ok(Self {
                path,
                operations: BTreeMap::new(),
            });
        };
        let mut bytes = Vec::new();
        std::io::Read::by_ref(&mut file)
            .take(MAX_WORKLOAD_LEDGER_BYTES + 1)
            .read_to_end(&mut bytes)?;
        validate_restart_ledger(&file)?;
        if bytes.len() as u64 > MAX_WORKLOAD_LEDGER_BYTES {
            return Err(WorkloadLedgerError::Malformed);
        }
        let body = std::str::from_utf8(&bytes).map_err(|_| WorkloadLedgerError::Malformed)?;
        reject_duplicate_json_keys(body).map_err(|_| WorkloadLedgerError::Malformed)?;
        let document: LedgerFile =
            serde_json::from_slice(&bytes).map_err(|_| WorkloadLedgerError::Malformed)?;
        if document.schema_version != WORKLOAD_CONTRACT_SCHEMA_VERSION {
            return Err(WorkloadLedgerError::Malformed);
        }
        if document.operations.len() > MAX_WORKLOAD_OPERATION_RECORDS {
            return Err(WorkloadLedgerError::Malformed);
        }
        let mut operations = BTreeMap::new();
        for record in document.operations {
            let request_id = record.request.request_id.clone();
            let validation_now = record
                .request
                .deadline_at_ms
                .checked_sub(1)
                .ok_or(WorkloadLedgerError::Malformed)?;
            record
                .request
                .validate(validation_now)
                .map_err(WorkloadLedgerError::Contract)?;
            // A historical status can contain an expired attachment lease; the
            // lease is revalidated at publication time, not while replaying the
            // journal after a restart.
            // Replay validates the lease shape without treating a historical
            // expiry as an invalid live publication.  Anchoring at one
            // millisecond before its expiry keeps the bounded-window check
            // meaningful while allowing leases that expired before restart.
            let status_validation_now = record
                .status
                .attachment
                .as_ref()
                .map(|lease| lease.expires_at_ms.saturating_sub(1))
                .unwrap_or(0);
            record
                .status
                .validate(status_validation_now)
                .map_err(WorkloadLedgerError::Contract)?;
            // The request and status are one durable authority record.  The
            // wire validators intentionally validate each object in
            // isolation, so replay must also reject a structurally valid
            // status that was paired with another request's identity or
            // placement contract.
            if record.status.request_id != record.request.request_id
                || record.status.workload_id != record.request.workload_id
                || record.status.backend != record.request.backend
                || record.status.resources != record.request.resources
                || record.status.image_ref != record.request.image_ref
            {
                return Err(WorkloadLedgerError::Malformed);
            }
            if operations.insert(request_id, record).is_some() {
                return Err(WorkloadLedgerError::Malformed);
            }
        }
        Ok(Self { path, operations })
    }

    /// Accept a request, persist `Queued`, and return the durable status.
    /// Repeating an identical request is a read-only idempotent replay.
    pub fn accept(
        &mut self,
        request: WorkloadOperationRequest,
        now_ms: u64,
    ) -> Result<WorkloadOperationStatus, WorkloadLedgerError> {
        request.validate(now_ms)?;
        if let Some(existing) = self.operations.get(&request.request_id) {
            if existing.request != request {
                return Err(WorkloadLedgerError::Conflict(request.request_id));
            }
            return Ok(existing.status.clone());
        }

        // Cancellation is a distinct journaled operation, but it advances the
        // targeted workload generation so the projection and capacity model
        // continue to expose one authoritative row.  It may coexist with its
        // target while cleanup runs; every other action remains exclusive.
        let cancellation_target_generation = if request.action == WorkloadOperationAction::Cancel {
            let target_id = request
                .target_request_id
                .as_deref()
                .ok_or_else(|| WorkloadLedgerError::UnknownRequest(request.request_id.clone()))?;
            let target = self
                .operations
                .get(target_id)
                .ok_or_else(|| WorkloadLedgerError::UnknownRequest(target_id.to_string()))?;
            if target.status.workload_id != request.workload_id
                || target.request.target_node != request.target_node
                || target.request.backend != request.backend
                || target.status.resources != request.resources
            {
                return Err(WorkloadLedgerError::Conflict(target_id.to_string()));
            }
            if request.expected_generation != target.status.generation {
                return Err(WorkloadLedgerError::StaleGeneration(
                    request.workload_id.as_str().to_string(),
                ));
            }
            Some(target.status.generation)
        } else {
            None
        };

        // The request is a compare-and-swap against the last durable
        // generation for this workload.  A zero generation is only valid for
        // the first operation; later requests must explicitly name the
        // current generation so stale GUI state fails closed.
        let latest_record = self
            .operations
            .values()
            .filter(|record| record.status.workload_id == request.workload_id)
            .max_by_key(|record| record.status.generation);
        if cancellation_target_generation.is_none() {
            if let Some(latest) = latest_record.filter(|record| !record.status.phase.is_terminal())
            {
                return Err(WorkloadLedgerError::Busy(latest.request.request_id.clone()));
            }
        }
        let generation = if let Some(target_generation) = cancellation_target_generation {
            target_generation.saturating_add(1)
        } else if let Some(latest) = latest_record.map(|record| record.status.generation) {
            if request.expected_generation == 0 || request.expected_generation != latest {
                return Err(WorkloadLedgerError::StaleGeneration(
                    request.workload_id.as_str().to_string(),
                ));
            }
            latest.saturating_add(1)
        } else {
            1
        };

        let status = WorkloadOperationStatus {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            request_id: request.request_id.clone(),
            workload_id: request.workload_id.clone(),
            backend: request.backend,
            resources: request.resources,
            image_ref: request.image_ref.clone(),
            generation,
            phase: WorkloadOperationPhase::Queued,
            power: mackes_mesh_types::workloads::WorkloadPowerState::Defined,
            readiness: mackes_mesh_types::workloads::WorkloadReadiness::Unknown,
            signals: mackes_mesh_types::workloads::WorkloadRuntimeSignals::default(),
            retryable: false,
            attempt: 0,
            next_retry_at_ms: 0,
            reason: None,
            remediation: None,
            attachment: None,
        };
        status.validate(now_ms)?;
        let previous_operations = self.operations.clone();
        self.operations.insert(
            request.request_id.clone(),
            LedgerRecord {
                request,
                status: status.clone(),
            },
        );
        if let Err(error) = self.prune_for_capacity() {
            self.operations = previous_operations;
            return Err(error);
        }
        if let Err(error) = self.flush() {
            self.operations = previous_operations;
            return Err(error);
        }
        Ok(status)
    }

    /// Persist one reconciler phase transition.  No side effect should be
    /// attempted until this method returns successfully for the preceding
    /// phase.
    pub fn advance(
        &mut self,
        request_id: &str,
        next: WorkloadOperationStatus,
        now_ms: u64,
    ) -> Result<WorkloadOperationStatus, WorkloadLedgerError> {
        next.validate(now_ms)?;
        let previous = {
            let record = self
                .operations
                .get_mut(request_id)
                .ok_or_else(|| WorkloadLedgerError::UnknownRequest(request_id.to_string()))?;
            if next.request_id != request_id
                || next.workload_id != record.status.workload_id
                || next.backend != record.status.backend
                || next.resources != record.status.resources
                || next.generation != record.status.generation
            {
                return Err(WorkloadLedgerError::Conflict(request_id.to_string()));
            }
            if !valid_phase_transition(record.status.phase, next.phase) {
                return Err(WorkloadLedgerError::InvalidTransition);
            }
            let previous = record.status.clone();
            record.status = next.clone();
            previous
        };
        if let Err(error) = self.flush() {
            // A failed atomic replacement must not leave this process ahead of
            // the durable journal. The next poll may retry, but it must retry
            // from the last state that survived a flush.
            if let Some(record) = self.operations.get_mut(request_id) {
                record.status = previous;
            }
            return Err(error);
        }
        Ok(next)
    }

    /// Return the last durable status for an idempotency key.
    #[must_use]
    pub fn status(&self, request_id: &str) -> Option<&WorkloadOperationStatus> {
        self.operations.get(request_id).map(|record| &record.status)
    }

    /// Return the original immutable request for an idempotency key.
    #[must_use]
    pub fn request(&self, request_id: &str) -> Option<&WorkloadOperationRequest> {
        self.operations
            .get(request_id)
            .map(|record| &record.request)
    }

    /// Return all durable operations in stable request-id order.
    #[must_use]
    pub fn statuses(&self) -> impl Iterator<Item = &WorkloadOperationStatus> {
        self.operations.values().map(|record| &record.status)
    }

    /// Evict only terminal history that is superseded by a newer generation.
    ///
    /// This is called after a new record has been inserted, so the new queued
    /// record itself makes the previous generation removable for that
    /// workload.  If no safe record exists, the caller rolls back the insert
    /// and no side effect can be started against an over-capacity journal.
    fn prune_for_capacity(&mut self) -> Result<(), WorkloadLedgerError> {
        while self.operations.len() > MAX_WORKLOAD_OPERATION_RECORDS {
            let mut latest_generation_by_workload = BTreeMap::new();
            for record in self.operations.values() {
                let workload_id = record.status.workload_id.as_str().to_string();
                let latest = latest_generation_by_workload
                    .entry(workload_id)
                    .or_insert(record.status.generation);
                *latest = (*latest).max(record.status.generation);
            }

            let mut candidates = self
                .operations
                .iter()
                .filter(|(_, record)| record.status.phase.is_terminal())
                .filter_map(|(request_id, record)| {
                    let latest =
                        latest_generation_by_workload.get(record.status.workload_id.as_str())?;
                    (record.status.generation < *latest)
                        .then(|| (request_id.clone(), record.status.generation))
                })
                .collect::<Vec<_>>();
            candidates.sort_by(|(left_id, left_generation), (right_id, right_generation)| {
                left_generation
                    .cmp(right_generation)
                    .then_with(|| left_id.cmp(right_id))
            });
            let Some((request_id, _)) = candidates.into_iter().next() else {
                return Err(WorkloadLedgerError::Capacity);
            };
            self.operations.remove(&request_id);
        }
        Ok(())
    }

    fn flush(&self) -> Result<(), WorkloadLedgerError> {
        let document = LedgerFile {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            operations: self.operations.values().cloned().collect(),
        };
        let bytes =
            serde_json::to_vec_pretty(&document).map_err(|_| WorkloadLedgerError::Malformed)?;
        let temp = self.path.with_extension("json.tmp");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, &self.path)?;
        // Syncing the parent makes the rename durable across a host crash on
        // filesystems that otherwise only guarantee the file's contents.
        if let Some(parent) = self.path.parent() {
            let directory = File::open(parent)?;
            directory.sync_all()?;
        }
        Ok(())
    }
}

/// Open the restart journal without allowing another filesystem name to share
/// or redirect the reconciler's lifecycle authority. Reading and validating
/// through this one descriptor also keeps a concurrent path replacement from
/// changing the bytes halfway through replay.
fn open_restart_ledger(path: &Path) -> Result<Option<File>, WorkloadLedgerError> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(0o400_000 | 0o4_000 | 0o2_000_000); // Linux O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    validate_restart_ledger(&file)?;
    Ok(Some(file))
}

fn validate_restart_ledger(file: &File) -> Result<(), WorkloadLedgerError> {
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file()
        || metadata.nlink() != 1
        || metadata.len() > MAX_WORKLOAD_LEDGER_BYTES
    {
        return Err(WorkloadLedgerError::Malformed);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::workloads::{
        WorkloadAttachmentProtocol, WorkloadOperationAction, WorkloadPowerState, WorkloadReadiness,
    };

    fn request(id: &str) -> WorkloadOperationRequest {
        WorkloadOperationRequest {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            request_id: id.to_string(),
            workload_id: mackes_mesh_types::workloads::WorkloadId::new("seat15-browser")
                .expect("id"),
            backend: mackes_mesh_types::workloads::WorkloadBackend::LibvirtVirtqemud,
            resources: mackes_mesh_types::workloads::WorkloadProfile::Small.resources(),
            image_ref: None,
            target_node: "seat15".into(),
            expected_generation: 0,
            action: WorkloadOperationAction::StartAndAttach,
            target_request_id: None,
            deadline_at_ms: 20_000,
            preferred_attachment: Some(WorkloadAttachmentProtocol::QemuDisplay1Dmabuf),
            armed_token: None,
        }
    }

    fn request_for(id: &str, workload_id: &str) -> WorkloadOperationRequest {
        let mut request = request(id);
        request.workload_id =
            mackes_mesh_types::workloads::WorkloadId::new(workload_id).expect("workload id");
        request
    }

    fn terminal_record(id: &str, generation: u64, workload_id: &str) -> LedgerRecord {
        let mut request = request_for(id, workload_id);
        request.expected_generation = generation.saturating_sub(1);
        LedgerRecord {
            status: WorkloadOperationStatus {
                schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
                request_id: request.request_id.clone(),
                workload_id: request.workload_id.clone(),
                backend: request.backend,
                resources: request.resources,
                image_ref: request.image_ref.clone(),
                generation,
                phase: WorkloadOperationPhase::Failed,
                power: WorkloadPowerState::Failed,
                readiness: WorkloadReadiness::Failed,
                signals: Default::default(),
                retryable: false,
                attempt: 0,
                next_retry_at_ms: 0,
                reason: Some("test completion".into()),
                remediation: None,
                attachment: None,
            },
            request,
        }
    }

    fn queued_record(id: &str, workload_id: &str) -> LedgerRecord {
        let request = request_for(id, workload_id);
        LedgerRecord {
            status: WorkloadOperationStatus {
                schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
                request_id: request.request_id.clone(),
                workload_id: request.workload_id.clone(),
                backend: request.backend,
                resources: request.resources,
                image_ref: request.image_ref.clone(),
                generation: 1,
                phase: WorkloadOperationPhase::Queued,
                power: WorkloadPowerState::Defined,
                readiness: WorkloadReadiness::Unknown,
                signals: Default::default(),
                retryable: false,
                attempt: 0,
                next_retry_at_ms: 0,
                reason: None,
                remediation: None,
                attachment: None,
            },
            request,
        }
    }

    fn write_document(root: &std::path::Path, records: Vec<LedgerRecord>) {
        let document = LedgerFile {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            operations: records,
        };
        let bytes = serde_json::to_vec(&document).expect("document");
        std::fs::write(root.join(WORKLOAD_LEDGER_FILENAME), bytes).expect("write journal");
    }

    #[test]
    fn accept_is_durable_and_idempotent_after_restart() {
        let temp = tempfile::tempdir().expect("temp");
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("open");
        let first = ledger.accept(request("req-1"), 1_000).expect("accept");
        let replay = ledger.accept(request("req-1"), 1_000).expect("replay");
        assert_eq!(first, replay);
        drop(ledger);
        let reopened = WorkloadOperationLedger::open(temp.path()).expect("reopen");
        assert_eq!(reopened.status("req-1"), Some(&first));
    }

    #[test]
    fn conflicting_reuse_and_illegal_transition_are_refused() {
        let temp = tempfile::tempdir().expect("temp");
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("open");
        ledger.accept(request("req-1"), 1_000).expect("accept");
        let mut conflict = request("req-1");
        conflict.action = WorkloadOperationAction::Stop;
        assert!(matches!(
            ledger.accept(conflict, 1_000),
            Err(WorkloadLedgerError::Conflict(_))
        ));
        let mut invalid = ledger.status("req-1").expect("status").clone();
        invalid.phase = WorkloadOperationPhase::Ready;
        invalid.power = WorkloadPowerState::Running;
        invalid.readiness = WorkloadReadiness::Ready;
        assert!(matches!(
            ledger.advance("req-1", invalid, 1_000),
            Err(WorkloadLedgerError::InvalidTransition)
        ));
    }

    #[test]
    fn valid_transition_is_written_before_the_next_actuator_step() {
        let temp = tempfile::tempdir().expect("temp");
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("open");
        ledger.accept(request("req-1"), 1_000).expect("accept");
        let mut next = ledger.status("req-1").expect("status").clone();
        next.phase = WorkloadOperationPhase::Validating;
        ledger.advance("req-1", next, 1_000).expect("advance");
        let reopened = WorkloadOperationLedger::open(temp.path()).expect("reopen");
        assert_eq!(
            reopened.status("req-1").expect("status").phase,
            WorkloadOperationPhase::Validating
        );
    }

    #[test]
    fn generation_compare_and_swap_rejects_stale_and_advances_matching_generation() {
        let temp = tempfile::tempdir().expect("temp");
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("open");
        ledger.accept(request("req-1"), 1_000).expect("first");
        let mut next = request("req-2");
        next.expected_generation = 1;
        assert!(matches!(
            ledger.accept(next.clone(), 1_000),
            Err(WorkloadLedgerError::Busy(_))
        ));
        let mut finished = ledger.status("req-1").expect("status").clone();
        finished.phase = WorkloadOperationPhase::Failed;
        finished.power = WorkloadPowerState::Failed;
        finished.readiness = WorkloadReadiness::Failed;
        finished.reason = Some("test completion".into());
        ledger.advance("req-1", finished, 1_000).expect("complete");
        let status = ledger.accept(next, 1_000).expect("next generation");
        assert_eq!(status.generation, 2);
        let mut finished = ledger.status("req-2").expect("status").clone();
        finished.phase = WorkloadOperationPhase::Failed;
        finished.power = WorkloadPowerState::Failed;
        finished.readiness = WorkloadReadiness::Failed;
        finished.reason = Some("test completion".into());
        ledger.advance("req-2", finished, 1_000).expect("complete");
        let mut stale = request("req-3");
        stale.expected_generation = 1;
        assert!(matches!(
            ledger.accept(stale, 1_000),
            Err(WorkloadLedgerError::StaleGeneration(_))
        ));
    }

    #[test]
    fn bounded_history_prunes_old_terminal_records_and_retains_latest_generation() {
        let temp = tempfile::tempdir().expect("temp");
        let records = (1..=MAX_WORKLOAD_OPERATION_RECORDS as u64)
            .map(|generation| {
                terminal_record(&format!("req-{generation}"), generation, "seat15-browser")
            })
            .collect();
        write_document(temp.path(), records);

        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("open");
        let next_id = format!("req-{}", MAX_WORKLOAD_OPERATION_RECORDS + 1);
        let mut next = request(&next_id);
        next.expected_generation = MAX_WORKLOAD_OPERATION_RECORDS as u64;
        let accepted = ledger.accept(next, 1_000).expect("next generation");
        assert_eq!(
            accepted.generation,
            MAX_WORKLOAD_OPERATION_RECORDS as u64 + 1
        );
        assert_eq!(ledger.statuses().count(), MAX_WORKLOAD_OPERATION_RECORDS);
        assert!(ledger.status("req-1").is_none());
        assert!(ledger.status("req-2").is_some());

        let retained = ledger.request("req-2").expect("retained request").clone();
        let replay = ledger.accept(retained, 1_000).expect("retained replay");
        assert_eq!(replay.request_id, "req-2");
        drop(ledger);
        let reopened = WorkloadOperationLedger::open(temp.path()).expect("reopen");
        assert_eq!(reopened.statuses().count(), MAX_WORKLOAD_OPERATION_RECORDS);
        assert_eq!(
            reopened.status(&next_id).expect("latest status").generation,
            MAX_WORKLOAD_OPERATION_RECORDS as u64 + 1
        );
    }

    #[test]
    fn full_active_journal_refuses_new_operation_without_mutation() {
        let temp = tempfile::tempdir().expect("temp");
        let records = (1..=MAX_WORKLOAD_OPERATION_RECORDS)
            .map(|index| queued_record(&format!("req-{index}"), &format!("workload-{index}")))
            .collect();
        write_document(temp.path(), records);

        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("open");
        let result = ledger.accept(request_for("req-over-capacity", "workload-new"), 1_000);
        assert!(matches!(result, Err(WorkloadLedgerError::Capacity)));
        assert_eq!(ledger.statuses().count(), MAX_WORKLOAD_OPERATION_RECORDS);
        assert!(ledger.status("req-over-capacity").is_none());
        drop(ledger);
        let reopened = WorkloadOperationLedger::open(temp.path()).expect("reopen");
        assert_eq!(reopened.statuses().count(), MAX_WORKLOAD_OPERATION_RECORDS);
        assert!(reopened.status("req-over-capacity").is_none());
    }

    #[test]
    fn oversized_persisted_journal_is_rejected_before_replay() {
        let temp = tempfile::tempdir().expect("temp");
        let records = (1..=MAX_WORKLOAD_OPERATION_RECORDS as u64 + 1)
            .map(|generation| {
                terminal_record(&format!("req-{generation}"), generation, "seat15-browser")
            })
            .collect();
        write_document(temp.path(), records);

        assert!(matches!(
            WorkloadOperationLedger::open(temp.path()),
            Err(WorkloadLedgerError::Malformed)
        ));
    }

    #[test]
    fn duplicate_persisted_keys_are_rejected_before_replay() {
        let temp = tempfile::tempdir().expect("temp");
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("open");
        ledger.accept(request("req-1"), 1_000).expect("accept");
        drop(ledger);

        let body = std::fs::read_to_string(temp.path().join(WORKLOAD_LEDGER_FILENAME))
            .expect("read journal");
        let hostile = body.replacen(
            "\"schema_version\": 1",
            "\"schema_version\": 1, \"schema_version\": 99",
            1,
        );
        std::fs::write(temp.path().join(WORKLOAD_LEDGER_FILENAME), hostile).expect("write hostile");

        assert!(matches!(
            WorkloadOperationLedger::open(temp.path()),
            Err(WorkloadLedgerError::Malformed)
        ));
    }

    #[test]
    fn recovered_status_must_remain_bound_to_its_request_authority() {
        let temp = tempfile::tempdir().expect("temp");
        let mut record = queued_record("req-1", "seat15-browser");
        record.status.workload_id =
            mackes_mesh_types::workloads::WorkloadId::new("seat16-browser").expect("id");
        write_document(temp.path(), vec![record]);

        assert!(matches!(
            WorkloadOperationLedger::open(temp.path()),
            Err(WorkloadLedgerError::Malformed)
        ));
    }

    #[test]
    fn restarted_reconciler_cannot_adopt_hardlinked_workload_journal_authority() {
        let temp = tempfile::tempdir().expect("temp");
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("open");
        ledger.accept(request("req-1"), 1_000).expect("accept");
        drop(ledger);

        let journal = temp.path().join(WORKLOAD_LEDGER_FILENAME);
        std::fs::hard_link(&journal, temp.path().join("external-ledger-alias.json"))
            .expect("create hostile journal alias");

        assert!(matches!(
            WorkloadOperationLedger::open(temp.path()),
            Err(WorkloadLedgerError::Malformed)
        ));
    }

    #[test]
    fn failed_atomic_transition_flush_rolls_back_in_memory_status() {
        let temp = tempfile::tempdir().expect("temp");
        let mut ledger = WorkloadOperationLedger::open(temp.path()).expect("open");
        let initial = ledger.accept(request("req-1"), 1_000).expect("accept");

        let journal = temp.path().join(WORKLOAD_LEDGER_FILENAME);
        std::fs::remove_file(&journal).expect("remove journal");
        std::fs::create_dir(&journal).expect("replace journal with directory");

        let mut next = initial.clone();
        next.phase = WorkloadOperationPhase::Validating;
        assert!(matches!(
            ledger.advance("req-1", next, 1_000),
            Err(WorkloadLedgerError::Io(_))
        ));
        assert_eq!(ledger.status("req-1"), Some(&initial));
    }
}
