//! VIRT-8.a (v5.0.0) — cold VM migration source-side worker.
//!
//! Each peer drains the single `action/compute/migrate` Bus topic.
//! For each request where `source_peer == own_nebula_ip`, the worker:
//!
//! 1. Requests graceful ACPI shutdown through the Workload migration adapter.
//! 2. Polls that adapter every 2 s until the domain is stopped or
//!    120 s timeout.
//! 3. `rsync --compress --progress <disk_path> <target>:<target_dir>`
//!    over the Nebula overlay.
//! 4. Publishes `event/compute/migrate-ready`; this worker's target-side
//!    adapter handoff defines the VM with the migrated disk + starts it. The source domain is left
//!    DEFINED-BUT-SHUTOFF as a rollback anchor.
//! 5. Waits for the target's `event/compute/migrate-committed` ack
//!    (correlated by request ULID, bounded by
//!    [`DEFAULT_COMMIT_TIMEOUT`]) and only THEN asks the adapter to relinquish the
//!    source-side definition. On a `migrate-failed` event or a commit
//!    timeout the source instead RE-DEFINES + re-starts the retained
//!    domain XML (rollback), so a failed migration never loses the VM
//!    — it stays runnable on the source (vdi-vm-5). `compute_registry`'s
//!    next 10 s tick publishes the updated `compute/inventory/<peer>`
//!    automatically (VIRT-8 bullet 3 satisfied without an explicit
//!    publish here).
//!
//! ## Topic-shape lock
//!
//! Design doc §3 notates the request topic as
//! `compute/migrate/<vm-id>`. Per Q96 + `rpc.rs`'s
//! `action/<domain>/<verb>` convention, the actual topic is
//! `action/compute/migrate` (single fixed topic), with per-peer
//! addressing in the payload's `source_peer` field. The migration's
//! correlation key is the request message's own ULID, propagated
//! into the published `event/compute/migrate-ready` so the target's
//! handler can correlate back. Followup in worklist
//! (VIRT-8.followup) to amend the design doc.
//!
//! Non-source peers see each message, advance the cursor, and skip
//! — the standard authenticated request/reply shape.

#![cfg(feature = "async-services")]

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mackes_mesh_types::cloud::{cloud_request_digest, CloudArmSigner, CloudArmedToken};
use mackes_mesh_types::workloads::reject_duplicate_json_keys;
use mde_bus::hooks::config::Priority;
use mde_bus::persist::{Persist, StoredMessage};

use crate::ipc::action_auth::{
    production_action_signer, ActionAuthorizer, MutationContext, ACTION_SCHEMA_VERSION,
    MAX_AUTH_TTL_MS,
};

use super::workload_compute::{WorkloadActuatorError, WorkloadMigrationClient};
use super::{ShutdownToken, Worker};

/// Bus action topic this worker drains.
pub const ACTION_TOPIC: &str = "action/compute/migrate";

/// Closed capability verb for the source-side migration request.  The
/// migration body is an administrative request: possession of the shared Bus
/// spool is transport reachability, not permission to stop a VM or ship its
/// disk to another peer.
pub const COMPUTE_MIGRATE_AUTH_VERB: &str = "compute-migrate";

/// Stable node scope used by the source-side capability.  The request's source
/// and destination peers are bound in [`migration_auth_target`], so a
/// capability for one migration route cannot be replayed on another route.
pub const COMPUTE_MIGRATE_NODE_SCOPE: &str = "compute";

/// Event capabilities are deliberately separate from the source request
/// capability.  A source request is consumed on the source host; a target
/// event and its source-side completion receipt must each be independently
/// armed by the root publisher.  Until a publisher supplies these envelopes,
/// the worker fails closed before every event-side backend call.
pub const COMPUTE_MIGRATE_READY_AUTH_VERB: &str = "compute-migrate-ready";
/// Capability verb for a target's successful migration receipt.
pub const COMPUTE_MIGRATE_COMMITTED_AUTH_VERB: &str = "compute-migrate-committed";
/// Capability verb for a target's migration-failure receipt.
pub const COMPUTE_MIGRATE_FAILED_AUTH_VERB: &str = "compute-migrate-failed";

/// Event topic published when the source side finishes shipping
/// the disk to the target. The target side (VIRT-8.b, same worker)
/// subscribes here + filters `target_peer == own`.
pub const MIGRATE_READY_TOPIC: &str = "event/compute/migrate-ready";

/// Event topic the target side publishes when it can't define/start
/// the migrated VM. It surfaces the failure to the operator UI AND
/// (vdi-vm-5) is consumed by the source side, which rolls the VM back
/// (re-defines + re-starts the retained domain) instead of leaving it
/// undefined and lost.
pub const MIGRATE_FAILED_TOPIC: &str = "event/compute/migrate-failed";

/// Event topic the target side publishes AFTER it has successfully
/// `virsh define`d + `virsh start`ed the migrated VM. It is the
/// source's signal that the destructive `virsh undefine` is now safe:
/// the source keeps its domain defined-but-shutoff until it observes
/// this ack (correlated by request ULID), so a target that never comes
/// up can never leave the VM lost (vdi-vm-5).
pub const MIGRATE_COMMITTED_TOPIC: &str = "event/compute/migrate-committed";

/// Default poll cadence — control surface.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(400);

/// Nebula overlay interface name (consistent with the rest of the
/// mackesd workers).
pub const DEFAULT_NEBULA_INTERFACE: &str = "nebula1";

/// Maximum wait for the guest to ACPI-shutdown before declaring the
/// migration failed (design doc §8 + task body bullet 1).
pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(120);

/// Inter-poll spacing for `virsh domstate` while waiting on
/// shutdown. 2 s balances responsiveness against virsh subprocess
/// churn.
pub const DEFAULT_SHUTDOWN_POLL: Duration = Duration::from_secs(2);

/// Target-side VM storage directory rsync ships disks into.
pub const DEFAULT_TARGET_VM_DIR: &str = "/var/lib/mde-vms/";

/// Generous-but-finite hard bound for the disk-ship `rsync`. A VM disk can be
/// many GiB and legitimately take minutes over the Nebula overlay, so this is
/// deliberately large; it exists only so a wedged rsync (a dead target peer, a
/// black-holed overlay) is killed rather than blocking forever. On expiry the
/// migration degrades to a [`MigrationOutcome::RsyncFailure`], exactly like a
/// non-zero rsync exit. (mackesd-02: the migration also runs off the async
/// runtime thread — see `run()` — so a slow ship can't starve the watchdog.)
pub const RSYNC_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Maximum the source waits for the target's `migrate-committed` ack
/// after publishing `migrate-ready`, before it treats the migration as
/// failed and ROLLS BACK (re-defines + re-starts the retained domain).
/// Generous — the target must drain migrate-ready, `virsh define`, boot
/// the guest, and publish the ack, all across the Nebula overlay — but
/// finite so a target that silently never comes up can't strand the
/// source's domain in the shut-off limbo forever (vdi-vm-5).
pub const DEFAULT_COMMIT_TIMEOUT: Duration = Duration::from_secs(180);

/// Root-only durable state for distributed migration admission and recovery.
pub const DEFAULT_MIGRATION_STATE_ROOT: &str = "/var/lib/mackesd/compute-migrate";
const MIGRATION_LEDGER_FILE: &str = "ledger.json";
const MIGRATION_LEDGER_SCHEMA_VERSION: u8 = 1;
const MAX_MIGRATION_LEDGER_BYTES: u64 = 8 * 1024 * 1024;
const MAX_MIGRATION_JOBS: usize = 128;
const MAX_MIGRATION_FIELD_BYTES: usize = 64 * 1024;
const MAX_MIGRATION_WIRE_BYTES: usize = 64 * 1024;
const MAX_MIGRATION_ID_BYTES: usize = 256;
const MAX_MIGRATION_PATH_BYTES: usize = 4 * 1024;
const MAX_MIGRATION_ERROR_BYTES: usize = 4 * 1024;
/// Bounds for startup Bus recovery. A late-mounted shared spool must recover
/// without a daemon restart, while a bad test/configuration cannot hot-loop.
const MIN_BUS_RETRY_INTERVAL: Duration = Duration::from_millis(10);
const MAX_BUS_RETRY_INTERVAL: Duration = Duration::from_secs(2);

/// Migration-request payload per design doc §3.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MigrateRequest {
    /// Source peer's Nebula overlay IP. Only the peer whose own
    /// nebula address matches this acts on the request.
    pub source_peer: String,
    /// Target peer's Nebula overlay IP. The rsync destination.
    pub target_peer: String,
    /// libvirt domain ID (UUID) of the VM being migrated.
    pub vm_id: String,
    /// Absolute path to the VM's primary disk on the source peer.
    pub disk_path: String,
}

/// `event/compute/migrate-ready` payload, published by the source
/// after a successful disk ship. The target side (VIRT-8.b) reads
/// `target_peer == own_nebula_ip` to claim responsibility, then
/// `virsh define`s `domain_xml` + starts the VM.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MigrateReadyEvent {
    /// Source peer's Nebula overlay IP (audit + Workbench display).
    pub source_peer: String,
    /// Target peer's Nebula overlay IP — the recipient filter.
    pub target_peer: String,
    /// VM id.
    pub vm_id: String,
    /// Absolute path the disk landed at on the target.
    pub target_disk_path: String,
    /// ULID of the originating `action/compute/migrate` request, so
    /// the target peer can correlate failures back to the operator.
    pub request_ulid: String,
    /// The source VM's `virsh dumpxml` output, captured BEFORE
    /// shutdown. The target `virsh define`s it verbatim — the disk
    /// `<source file=…>` path matches on both peers (identical
    /// `/var/lib/mde-vms/<vm-id>.qcow2` pool layout), and the VM's
    /// Nebula identity lives in the disk, so the migrated VM keeps
    /// its full config (network, virtiofs, memory) + cert.
    #[serde(default)]
    pub domain_xml: String,
}

/// `event/compute/migrate-failed` payload (target-side).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MigrateFailedEvent {
    /// VM id that failed to come up on the target.
    pub vm_id: String,
    /// Target peer that couldn't define/start it.
    pub target_peer: String,
    /// Correlation ULID of the original migrate request.
    pub request_ulid: String,
    /// Human-readable failure description.
    pub error: String,
}

/// `event/compute/migrate-committed` payload (target-side). Published
/// only after a SUCCESSFUL define+start, it is the source's cue that
/// the deferred `virsh undefine` is now safe to run (vdi-vm-5).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MigrateCommittedEvent {
    /// VM id now running on the target.
    pub vm_id: String,
    /// Source peer still holding the shut-off rollback anchor — the
    /// recipient the ack is addressed to (audit; correlation is by ULID).
    pub source_peer: String,
    /// Target peer that brought the VM up.
    pub target_peer: String,
    /// Correlation ULID of the original migrate request — the source
    /// matches this against its pending commits.
    pub request_ulid: String,
}

/// Reason the source rolled a migration back instead of undefining.
#[derive(Debug, Clone, PartialEq)]
pub enum RollbackReason {
    /// Target published `migrate-failed` — it couldn't define/start.
    TargetFailed {
        /// The target's failure description.
        error: String,
    },
    /// No `migrate-committed` (nor `migrate-failed`) arrived within
    /// [`DEFAULT_COMMIT_TIMEOUT`].
    CommitTimeout,
}

/// How a source-side pending commit resolves on a given tick.
#[derive(Debug, Clone, PartialEq)]
pub enum CommitResolution {
    /// Target confirmed the VM is running → the source may now run the
    /// deferred `virsh undefine`.
    Undefine,
    /// Migration failed or the ack timed out → the source re-defines +
    /// re-starts the retained domain (rollback), so the VM is not lost.
    RollBack {
        /// Why the source is rolling back.
        reason: RollbackReason,
    },
    /// Neither ack nor timeout yet — keep waiting.
    Pending,
}

/// Outcome of the source-side migration flow.
#[derive(Debug, Clone, PartialEq)]
pub enum MigrationOutcome {
    /// Disk landed on the target; the source domain is left
    /// DEFINED-BUT-SHUTOFF as the rollback anchor. Carries the captured
    /// `virsh dumpxml` so the caller can include it in migrate-ready AND
    /// retain it to roll back / undefine once the target acks (vdi-vm-5).
    Ok {
        /// `virsh dumpxml` output captured before shutdown.
        domain_xml: String,
    },
    /// Guest didn't ACPI-shutdown within
    /// [`DEFAULT_SHUTDOWN_TIMEOUT`].
    ShutdownTimeout,
    /// `rsync` returned a non-zero exit status.
    RsyncFailure {
        /// Description of the rsync failure.
        exit_description: String,
    },
    /// The sole Workload migration adapter refused or could not complete a
    /// definition/power operation.
    AuthorityFailure {
        /// Bounded adapter failure description.
        reason: String,
    },
}

trait MigrationAuthority: Send + Sync {
    fn capture_definition(&self, vm_id: &str) -> Result<String, WorkloadActuatorError>;
    fn request_stop(&self, vm_id: &str) -> Result<(), WorkloadActuatorError>;
    fn is_stopped(&self, vm_id: &str) -> Result<bool, WorkloadActuatorError>;
    fn define_and_start(&self, vm_id: &str, domain_xml: &str) -> Result<(), WorkloadActuatorError>;
    fn relinquish_definition(&self, vm_id: &str) -> Result<(), WorkloadActuatorError>;
}

impl MigrationAuthority for WorkloadMigrationClient {
    fn capture_definition(&self, vm_id: &str) -> Result<String, WorkloadActuatorError> {
        (*self).capture_definition(vm_id)
    }

    fn request_stop(&self, vm_id: &str) -> Result<(), WorkloadActuatorError> {
        (*self).request_stop(vm_id)
    }

    fn is_stopped(&self, vm_id: &str) -> Result<bool, WorkloadActuatorError> {
        (*self).is_stopped(vm_id)
    }

    fn define_and_start(&self, vm_id: &str, domain_xml: &str) -> Result<(), WorkloadActuatorError> {
        (*self).define_and_start(vm_id, domain_xml)
    }

    fn relinquish_definition(&self, vm_id: &str) -> Result<(), WorkloadActuatorError> {
        (*self).relinquish_definition(vm_id)
    }
}

/// Parse a migration-request body.
///
/// # Errors
///
/// Returns a human-readable error string on malformed JSON or
/// missing required fields.
pub fn parse_migrate_request(body: &str) -> Result<MigrateRequest, String> {
    validate_wire_body(body, "migrate request")?;
    let request: MigrateRequest = serde_json::from_str(body)
        .map_err(|error| format!("malformed migrate request: {error}"))?;
    validate_peer(&request.source_peer)?;
    validate_peer(&request.target_peer)?;
    if request.source_peer == request.target_peer {
        return Err("migration source and target must differ".into());
    }
    validate_migration_id(&request.vm_id, "VM identity")?;
    validate_managed_disk_path(&request.disk_path)?;
    Ok(request)
}

fn validate_wire_body(body: &str, label: &str) -> Result<(), String> {
    if body.len() > MAX_MIGRATION_WIRE_BYTES {
        return Err(format!("{label} exceeds the wire byte limit"));
    }
    reject_duplicate_json_keys(body)
        .map_err(|_| format!("{label} is malformed or contains duplicate JSON keys"))
}

fn validate_peer(peer: &str) -> Result<(), String> {
    if peer.len() > 64 || peer.parse::<std::net::IpAddr>().is_err() {
        return Err("migration peer is not a bounded IP address".into());
    }
    Ok(())
}

fn validate_migration_id(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_MIGRATION_ID_BYTES
        || value.chars().any(char::is_control)
        || value.contains('/')
        || value == "."
        || value == ".."
    {
        return Err(format!("migration {label} is invalid"));
    }
    Ok(())
}

fn validate_managed_disk_path(value: &str) -> Result<(), String> {
    let path = Path::new(value);
    if value.len() > MAX_MIGRATION_PATH_BYTES
        || !path.is_absolute()
        || !path.starts_with(DEFAULT_TARGET_VM_DIR)
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
        || path.file_name().is_none()
    {
        return Err("migration disk path is outside the managed VM directory".into());
    }
    Ok(())
}

/// Stable capability target for a source-side migration.  Keep the source and
/// destination peers in the target so a capability cannot be replayed for the
/// same VM on a different migration route.  The exact raw body is also bound by
/// [`ActionAuthorizer`], so this is a semantic second check rather than a body
/// replacement.
#[must_use]
pub fn migration_auth_target(req: &MigrateRequest) -> String {
    format!(
        "vm:{}:{}->{}",
        req.vm_id.trim(),
        req.source_peer.trim(),
        req.target_peer.trim()
    )
}

/// Stable capability target shared by the ready/committed/failed event lanes.
#[must_use]
fn migration_event_auth_target(vm_id: &str, source_peer: &str, target_peer: &str) -> String {
    format!(
        "vm:{}:{}->{}",
        vm_id.trim(),
        source_peer.trim(),
        target_peer.trim()
    )
}

/// Verify an event's exact raw envelope before any event-side mutation.  Event
/// bodies intentionally carry their own schema-v1 `armed_token`; reusing the
/// source action token would either permit replay or fail on the source host's
/// already-spent nonce, so an explicit event publisher/key handoff is required.
fn authorize_event_body(
    authorizer: &ActionAuthorizer,
    body: &str,
    verb: &str,
    vm_id: &str,
    source_peer: &str,
    target_peer: &str,
) -> Result<(), String> {
    let target = migration_event_auth_target(vm_id, source_peer, target_peer);
    authorizer.authorize(
        body,
        MutationContext {
            verb,
            node: COMPUTE_MIGRATE_NODE_SCOPE,
            target: &target,
        },
    )
}

/// Verify the source-side request before the migration runner can invoke
/// `virsh`, `rsync`, or any other backend.  Parsing and source-peer routing are
/// deliberately performed before this call; they are pure and do not consume a
/// capability nonce on peers that are not the request's source.
fn authorize_source_request(
    authorizer: &ActionAuthorizer,
    body: &str,
    req: &MigrateRequest,
) -> Result<(), String> {
    let target = migration_auth_target(req);
    authorizer.authorize(
        body,
        MutationContext {
            verb: COMPUTE_MIGRATE_AUTH_VERB,
            node: COMPUTE_MIGRATE_NODE_SCOPE,
            target: &target,
        },
    )
}

/// `true` when this peer is the source for the request.
#[must_use]
pub fn is_source_peer(req: &MigrateRequest, own_nebula_ip: &str) -> bool {
    !own_nebula_ip.is_empty() && req.source_peer == own_nebula_ip
}

/// `true` when this peer is the target for a migrate-ready event.
#[must_use]
pub fn is_target_peer(event: &MigrateReadyEvent, own_nebula_ip: &str) -> bool {
    !own_nebula_ip.is_empty() && event.target_peer == own_nebula_ip
}

/// Parse a migrate-ready event body.
///
/// # Errors
///
/// Returns a human-readable error on malformed JSON.
pub fn parse_migrate_ready_event(body: &str) -> Result<MigrateReadyEvent, String> {
    validate_wire_body(body, "migrate-ready event")?;
    let event: MigrateReadyEvent = serde_json::from_str(body)
        .map_err(|error| format!("malformed migrate-ready event: {error}"))?;
    validate_peer(&event.source_peer)?;
    validate_peer(&event.target_peer)?;
    validate_migration_id(&event.vm_id, "VM identity")?;
    validate_migration_id(&event.request_ulid, "request identity")?;
    validate_managed_disk_path(&event.target_disk_path)?;
    if event.domain_xml.trim().is_empty() || event.domain_xml.len() > MAX_MIGRATION_FIELD_BYTES {
        return Err("migration retained definition is invalid".into());
    }
    Ok(event)
}

/// Build the `rsync --compress` args for shipping a disk from the
/// source to the target peer's `/var/lib/mde-vms/`. SSH is used
/// implicitly (rsync's default remote-shell), which over Nebula
/// goes via the peer's overlay-bound sshd (NF-21.1).
#[must_use]
pub fn build_rsync_args(disk_path: &str, target_peer: &str, target_dir: &str) -> Vec<String> {
    let dest = format!("{target_peer}:{target_dir}");
    vec![
        "--compress".into(),
        "--progress".into(),
        disk_path.into(),
        dest,
    ]
}

/// Compute the expected target-side path after the rsync. rsync
/// preserves the source filename, so target_disk_path is just
/// `<target_dir>/<basename>`.
#[must_use]
pub fn target_disk_path_for(disk_path: &str, target_dir: &str) -> String {
    let basename = std::path::Path::new(disk_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("disk.qcow2");
    let sep = if target_dir.ends_with('/') { "" } else { "/" };
    format!("{target_dir}{sep}{basename}")
}

/// Build the `event/compute/migrate-ready` payload.
#[must_use]
pub fn build_migrate_ready_event(
    req: &MigrateRequest,
    target_disk_path: String,
    request_ulid: String,
    domain_xml: String,
) -> MigrateReadyEvent {
    MigrateReadyEvent {
        source_peer: req.source_peer.clone(),
        target_peer: req.target_peer.clone(),
        vm_id: req.vm_id.clone(),
        target_disk_path,
        request_ulid,
        domain_xml,
    }
}

/// Parse a migrate-failed event body (source side consumes these to
/// roll back — vdi-vm-5).
///
/// # Errors
///
/// Returns a human-readable error on malformed JSON.
pub fn parse_migrate_failed_event(body: &str) -> Result<MigrateFailedEvent, String> {
    validate_wire_body(body, "migrate-failed event")?;
    let event: MigrateFailedEvent = serde_json::from_str(body)
        .map_err(|error| format!("malformed migrate-failed event: {error}"))?;
    validate_peer(&event.target_peer)?;
    validate_migration_id(&event.vm_id, "VM identity")?;
    validate_migration_id(&event.request_ulid, "request identity")?;
    if event.error.is_empty()
        || event.error.len() > MAX_MIGRATION_ERROR_BYTES
        || event.error.chars().any(char::is_control)
    {
        return Err("migration failure description is invalid".into());
    }
    Ok(event)
}

/// Parse a migrate-committed event body.
///
/// # Errors
///
/// Returns a human-readable error on malformed JSON.
pub fn parse_migrate_committed_event(body: &str) -> Result<MigrateCommittedEvent, String> {
    validate_wire_body(body, "migrate-committed event")?;
    let event: MigrateCommittedEvent = serde_json::from_str(body)
        .map_err(|error| format!("malformed migrate-committed event: {error}"))?;
    validate_peer(&event.source_peer)?;
    validate_peer(&event.target_peer)?;
    validate_migration_id(&event.vm_id, "VM identity")?;
    validate_migration_id(&event.request_ulid, "request identity")?;
    Ok(event)
}

/// Build the `event/compute/migrate-committed` payload from the
/// migrate-ready event the target just provisioned, preserving the
/// correlation ULID so the source can match its pending commit.
#[must_use]
pub fn build_migrate_committed_event(event: &MigrateReadyEvent) -> MigrateCommittedEvent {
    MigrateCommittedEvent {
        vm_id: event.vm_id.clone(),
        source_peer: event.source_peer.clone(),
        target_peer: event.target_peer.clone(),
        request_ulid: event.request_ulid.clone(),
    }
}

/// Pure resolver for a source-side pending commit: decide, from the
/// ULIDs observed committed, the ULIDs observed failed (with their
/// error text), and whether the commit deadline has passed, whether the
/// source should undefine, roll back, or keep waiting (vdi-vm-5).
///
/// Precedence: a `migrate-committed` wins (the VM is confirmed up on the
/// target, so the undefine is safe even if a stale failure was also
/// seen); then a `migrate-failed`; then the timeout. The run loop
/// supplies `timed_out` from the clock, so the decision core is
/// deterministically testable without wall-clock waits.
#[must_use]
pub fn classify_commit(
    request_ulid: &str,
    committed_ulids: &[String],
    failed: &[(String, String)],
    timed_out: bool,
) -> CommitResolution {
    if committed_ulids.iter().any(|u| u == request_ulid) {
        return CommitResolution::Undefine;
    }
    if let Some((_, error)) = failed.iter().find(|(u, _)| u == request_ulid) {
        return CommitResolution::RollBack {
            reason: RollbackReason::TargetFailed {
                error: error.clone(),
            },
        };
    }
    if timed_out {
        return CommitResolution::RollBack {
            reason: RollbackReason::CommitTimeout,
        };
    }
    CommitResolution::Pending
}

fn run_rsync(args: &[String]) -> Result<(), String> {
    let mut cmd = Command::new("rsync");
    cmd.args(args);
    // Bounded so a wedged rsync (dead peer / black-holed overlay) is killed at
    // RSYNC_TIMEOUT instead of blocking indefinitely (mackesd-02).
    match super::proc::status_with_timeout(cmd, RSYNC_TIMEOUT) {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("rsync exited {status}")),
        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => Err(format!(
            "rsync timed out after {}s",
            RSYNC_TIMEOUT.as_secs()
        )),
        Err(e) => Err(format!("rsync spawn: {e}")),
    }
}

fn local_nebula_addr(interface: &str) -> String {
    let Ok(output) = Command::new("ip")
        .args(["-4", "addr", "show", interface])
        .output()
    else {
        return String::new();
    };
    if !output.status.success() {
        return String::new();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("inet ") {
            if let Some(ip) = rest.split('/').next() {
                return ip.to_string();
            }
        }
    }
    String::new()
}

/// Drive the source-side migration flow for one request. Returns the
/// terminal outcome. VM lifecycle calls cross the Workload migration adapter;
/// the disk-copy subprocess remains here. The timeout uses [`DEFAULT_SHUTDOWN_TIMEOUT`] /
/// [`DEFAULT_SHUTDOWN_POLL`] under the hood.
fn run_migration(req: &MigrateRequest, actuator: &dyn MigrationAuthority) -> MigrationOutcome {
    // Step 0: capture the domain XML WHILE the VM is still defined,
    // so the target can recreate it verbatim. Empty on failure — the
    // target handler surfaces a clear migrate-failed in that case.
    let domain_xml = match actuator.capture_definition(&req.vm_id) {
        Ok(xml) => xml,
        Err(error) => {
            return MigrationOutcome::AuthorityFailure {
                reason: error.to_string(),
            };
        }
    };

    // Step 1: ACPI shutdown.
    if let Err(error) = actuator.request_stop(&req.vm_id) {
        let outcome = MigrationOutcome::AuthorityFailure {
            reason: error.to_string(),
        };
        return restore_source_after_failed_migration(req, &domain_xml, actuator, outcome);
    }

    // Step 2: poll for shutoff.
    let attempts =
        (DEFAULT_SHUTDOWN_TIMEOUT.as_millis() / DEFAULT_SHUTDOWN_POLL.as_millis()) as usize;
    let mut shutoff = false;
    for _ in 0..attempts {
        std::thread::sleep(DEFAULT_SHUTDOWN_POLL);
        match actuator.is_stopped(&req.vm_id) {
            Ok(true) => {
                shutoff = true;
                break;
            }
            Ok(false) => {}
            Err(error) => {
                let outcome = MigrationOutcome::AuthorityFailure {
                    reason: error.to_string(),
                };
                return restore_source_after_failed_migration(req, &domain_xml, actuator, outcome);
            }
        }
    }
    if !shutoff {
        return restore_source_after_failed_migration(
            req,
            &domain_xml,
            actuator,
            MigrationOutcome::ShutdownTimeout,
        );
    }

    // Step 3: rsync.
    let rsync_args = build_rsync_args(&req.disk_path, &req.target_peer, DEFAULT_TARGET_VM_DIR);
    if let Err(e) = run_rsync(&rsync_args) {
        let outcome = MigrationOutcome::RsyncFailure {
            exit_description: e,
        };
        return restore_source_after_failed_migration(req, &domain_xml, actuator, outcome);
    }

    // NOTE (vdi-vm-5): the source-side `virsh undefine` is DEFERRED. The
    // domain stays DEFINED-BUT-SHUTOFF here as the rollback anchor; the
    // run loop only undefines after the target acks with
    // `migrate-committed`, and re-defines + re-starts from the retained
    // `domain_xml` on a `migrate-failed` or a commit timeout. Publish of
    // migrate-ready also happens in the caller so it can carry the
    // request_ulid.
    MigrationOutcome::Ok { domain_xml }
}

fn restore_source_after_failed_migration(
    req: &MigrateRequest,
    domain_xml: &str,
    actuator: &dyn MigrationAuthority,
    outcome: MigrationOutcome,
) -> MigrationOutcome {
    match actuator.define_and_start(&req.vm_id, domain_xml) {
        Ok(()) => outcome,
        Err(error) => MigrationOutcome::AuthorityFailure {
            reason: format!("migration failed and source recovery failed: {error}"),
        },
    }
}

/// VIRT-8.b — target-side: define + start the migrated VM from the
/// captured XML. The disk is already in place (rsync'd by the
/// source) at the matching `/var/lib/mde-vms/<vm>.qcow2` path the
/// XML references, so a verbatim `virsh define` + `virsh start`
/// recreates the VM with its full config + Nebula identity.
///
/// # Errors
///
/// Returns a description when virsh is absent, the XML is empty
/// (source dumpxml failed), or define/start exits non-zero.
fn run_migrate_target(
    event: &MigrateReadyEvent,
    actuator: &dyn MigrationAuthority,
) -> Result<(), String> {
    actuator
        .define_and_start(&event.vm_id, &event.domain_xml)
        .map_err(|error| error.to_string())
}

/// vdi-vm-5 — source-side rollback: re-define + re-start the retained
/// domain so a failed or timed-out migration leaves the VM runnable on
/// the source instead of lost. `virsh define` is a define-or-update, so
/// this is safe whether the shut-off anchor still exists or not.
///
/// # Errors
///
/// Returns the Workload adapter's bounded failure description.
fn run_source_rollback(
    vm_id: &str,
    domain_xml: &str,
    actuator: &dyn MigrationAuthority,
) -> Result<(), String> {
    actuator
        .define_and_start(vm_id, domain_xml)
        .map_err(|error| error.to_string())
}

/// vdi-vm-5 — the DEFERRED destructive step: remove the source-side
/// definition, run only once the target has acked with
/// `migrate-committed`. Returns whether virsh reported success.
fn run_source_undefine(vm_id: &str, actuator: &dyn MigrationAuthority) -> Result<(), String> {
    actuator
        .relinquish_definition(vm_id)
        .map_err(|error| error.to_string())
}

fn build_authorized_migrate_event_body<T: serde::Serialize>(
    event: &T,
    verb: &str,
    target: &str,
    signer: &CloudArmSigner,
    nonce: &str,
    now_ms: i64,
) -> Result<String, String> {
    let mut document = serde_json::to_value(event)
        .map_err(|error| format!("serialize migration event: {error}"))?;
    if !document.is_object() {
        return Err("migration event must serialize as a JSON object".into());
    }
    document["schema_version"] = serde_json::Value::from(ACTION_SCHEMA_VERSION);
    let unsigned = document.to_string();
    let token = CloudArmedToken::mint(
        signer,
        nonce,
        now_ms.saturating_add(MAX_AUTH_TTL_MS),
        verb,
        COMPUTE_MIGRATE_NODE_SCOPE,
        target,
        &cloud_request_digest(&unsigned).map_err(str::to_string)?,
    )
    .encode();
    document["armed_token"] = serde_json::Value::String(token);
    Ok(document.to_string())
}

fn publish_authorized_migrate_event<T: serde::Serialize>(
    persist: &Persist,
    topic: &str,
    event: &T,
    verb: &str,
    target: &str,
    signer: &CloudArmSigner,
    nonce: &str,
    now_ms: i64,
) -> Result<(), String> {
    let body = build_authorized_migrate_event_body(event, verb, target, signer, nonce, now_ms)?;
    persist
        .write(topic, Priority::Default, None, Some(&body))
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn production_migrate_event_credentials() -> Result<(CloudArmSigner, String, i64), String> {
    let signer = production_action_signer()?;
    let nonce = uuid::Uuid::new_v4().to_string();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "system clock is before the Unix epoch".to_string())
        .and_then(|duration| {
            i64::try_from(duration.as_millis())
                .map_err(|_| "system clock is beyond the capability range".to_string())
        })?;
    Ok((signer, nonce, now_ms))
}

fn build_production_migrate_event_body<T: serde::Serialize>(
    event: &T,
    verb: &str,
    target: &str,
) -> Result<String, String> {
    let (signer, nonce, now_ms) = production_migrate_event_credentials()?;
    build_authorized_migrate_event_body(event, verb, target, &signer, &nonce, now_ms)
}

fn publish_migration_body(persist: &Persist, topic: &str, body: &str) -> Result<(), String> {
    persist
        .write(topic, Priority::Default, None, Some(body))
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// A migration whose disk shipped + `migrate-ready` published, now
/// awaiting the target's `migrate-committed` ack. Holds the retained
/// `virsh dumpxml` so the source can roll back (re-define + re-start) if
/// the target fails or the ack times out (vdi-vm-5). The source domain
/// stays defined-but-shutoff until this resolves.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingCommit {
    request_ulid: String,
    vm_id: String,
    domain_xml: String,
    ready_event: MigrateReadyEvent,
    #[serde(default)]
    ready_body: Option<String>,
    deadline_ms: i64,
    phase: PendingPhase,
    next_attempt_ms: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum PendingPhase {
    PublishReady,
    Waiting,
    Relinquish,
    Relinquishing,
    Rollback { reason: String },
    RollingBack { reason: String },
    Indeterminate { operation: String, reason: String },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DurableSourceJob {
    message_ulid: String,
    raw_body: String,
    request: MigrateRequest,
    authorized: bool,
    #[serde(default)]
    effect_claimed: bool,
    #[serde(default)]
    failure: Option<DurableSourceFailure>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DurableSourceFailure {
    error: String,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DurableTargetJob {
    message_ulid: String,
    raw_body: String,
    event: MigrateReadyEvent,
    phase: TargetJobPhase,
    #[serde(default)]
    reply_body: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum TargetJobPhase {
    Prepared,
    Apply,
    Applying,
    PublishCommitted,
    PublishFailed { error: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BusIndexIdentity {
    device: u64,
    inode: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum DurableAckEvent {
    Committed { event: MigrateCommittedEvent },
    Failed { event: MigrateFailedEvent },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DurableAckJob {
    message_ulid: String,
    raw_body: String,
    event: DurableAckEvent,
    authorized: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MigrationLedgerState {
    schema_version: u8,
    #[serde(default)]
    bus_identity: Option<BusIndexIdentity>,
    source_cursor: Option<String>,
    target_cursor: Option<String>,
    committed_cursor: Option<String>,
    failed_cursor: Option<String>,
    source_jobs: Vec<DurableSourceJob>,
    target_jobs: Vec<DurableTargetJob>,
    ack_jobs: Vec<DurableAckJob>,
    pending_commits: Vec<PendingCommit>,
}

impl Default for MigrationLedgerState {
    fn default() -> Self {
        Self {
            schema_version: MIGRATION_LEDGER_SCHEMA_VERSION,
            bus_identity: None,
            source_cursor: None,
            target_cursor: None,
            committed_cursor: None,
            failed_cursor: None,
            source_jobs: Vec::new(),
            target_jobs: Vec::new(),
            ack_jobs: Vec::new(),
            pending_commits: Vec::new(),
        }
    }
}

struct MigrationLedger {
    root: PathBuf,
    state: MigrationLedgerState,
}

impl MigrationLedgerState {
    fn validate(&self) -> Result<(), String> {
        if self.schema_version != MIGRATION_LEDGER_SCHEMA_VERSION
            || self.source_jobs.len() > MAX_MIGRATION_JOBS
            || self.target_jobs.len() > MAX_MIGRATION_JOBS
            || self.ack_jobs.len() > MAX_MIGRATION_JOBS
            || self.pending_commits.len() > MAX_MIGRATION_JOBS
        {
            return Err("migration ledger violates schema or capacity bounds".into());
        }
        let valid = |value: &str| !value.is_empty() && value.len() <= MAX_MIGRATION_FIELD_BYTES;
        for cursor in [
            &self.source_cursor,
            &self.target_cursor,
            &self.committed_cursor,
            &self.failed_cursor,
        ] {
            if cursor.as_deref().is_some_and(|value| !valid(value)) {
                return Err("migration ledger contains an invalid cursor".into());
            }
        }
        let mut identities = BTreeSet::new();
        for job in &self.source_jobs {
            if !valid(&job.message_ulid)
                || !valid(&job.raw_body)
                || !valid(&job.request.vm_id)
                || !valid(&job.request.source_peer)
                || !valid(&job.request.target_peer)
                || !valid(&job.request.disk_path)
                || job.failure.as_ref().is_some_and(|failure| {
                    !valid(&failure.error)
                        || failure.body.as_deref().is_some_and(|body| !valid(body))
                })
            {
                return Err("migration ledger contains an invalid source job".into());
            }
            if !identities.insert(("source", job.message_ulid.as_str())) {
                return Err("migration ledger contains a duplicate source job".into());
            }
        }
        for job in &self.target_jobs {
            if !valid(&job.message_ulid)
                || !valid(&job.raw_body)
                || !valid(&job.event.vm_id)
                || !valid(&job.event.request_ulid)
                || !valid(&job.event.domain_xml)
                || job.reply_body.as_deref().is_some_and(|body| !valid(body))
            {
                return Err("migration ledger contains an invalid target job".into());
            }
            if !identities.insert(("target", job.message_ulid.as_str())) {
                return Err("migration ledger contains a duplicate target job".into());
            }
        }
        for pending in &self.pending_commits {
            if !valid(&pending.request_ulid)
                || !valid(&pending.vm_id)
                || !valid(&pending.domain_xml)
                || pending
                    .ready_body
                    .as_deref()
                    .is_some_and(|body| !valid(body))
                || pending.deadline_ms < 0
                || pending.next_attempt_ms < 0
            {
                return Err("migration ledger contains an invalid pending commit".into());
            }
            if !identities.insert(("pending", pending.request_ulid.as_str())) {
                return Err("migration ledger contains a duplicate pending commit".into());
            }
        }
        for ack in &self.ack_jobs {
            if !valid(&ack.message_ulid) || !valid(&ack.raw_body) {
                return Err("migration ledger contains an invalid acknowledgement".into());
            }
            if !identities.insert(("ack", ack.message_ulid.as_str())) {
                return Err("migration ledger contains a duplicate acknowledgement".into());
            }
        }
        Ok(())
    }
}

impl MigrationLedger {
    fn open(root: &Path) -> Result<Self, String> {
        fs::create_dir_all(root)
            .map_err(|error| format!("create migration state root: {error}"))?;
        let metadata = fs::symlink_metadata(root)
            .map_err(|error| format!("inspect migration state root: {error}"))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("migration state root is not a regular directory".into());
        }
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("secure migration state root: {error}"))?;
        let path = root.join(MIGRATION_LEDGER_FILE);
        let (state, existed) = match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if !metadata.is_file()
                    || metadata.file_type().is_symlink()
                    || metadata.len() > MAX_MIGRATION_LEDGER_BYTES
                {
                    return Err("migration ledger is not a bounded regular file".into());
                }
                let mut file: fs::File = rustix::fs::open(
                    &path,
                    rustix::fs::OFlags::RDONLY
                        | rustix::fs::OFlags::NOFOLLOW
                        | rustix::fs::OFlags::NONBLOCK
                        | rustix::fs::OFlags::CLOEXEC,
                    rustix::fs::Mode::empty(),
                )
                .map_err(|error| format!("open migration ledger: {error}"))?
                .into();
                let mut body = Vec::with_capacity(metadata.len() as usize);
                std::io::Read::by_ref(&mut file)
                    .take(MAX_MIGRATION_LEDGER_BYTES.saturating_add(1))
                    .read_to_end(&mut body)
                    .map_err(|error| format!("read migration ledger: {error}"))?;
                if body.len() as u64 > MAX_MIGRATION_LEDGER_BYTES {
                    return Err("migration ledger exceeds its byte bound".into());
                }
                let text = std::str::from_utf8(&body)
                    .map_err(|_| "migration ledger is not UTF-8".to_string())?;
                reject_duplicate_json_keys(text)
                    .map_err(|_| "migration ledger contains duplicate JSON keys".to_string())?;
                (
                    serde_json::from_slice(&body)
                        .map_err(|error| format!("decode migration ledger: {error}"))?,
                    true,
                )
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (MigrationLedgerState::default(), false)
            }
            Err(error) => return Err(format!("inspect migration ledger: {error}")),
        };
        state.validate()?;
        let ledger = Self {
            root: root.to_path_buf(),
            state,
        };
        if !existed {
            ledger.store()?;
        }
        Ok(ledger)
    }

    fn store(&self) -> Result<(), String> {
        self.state.validate()?;
        let body = serde_json::to_vec(&self.state)
            .map_err(|error| format!("encode migration ledger: {error}"))?;
        if body.len() as u64 > MAX_MIGRATION_LEDGER_BYTES {
            return Err("migration ledger exceeds its byte bound".into());
        }
        let destination = self.root.join(MIGRATION_LEDGER_FILE);
        let temporary = self.root.join(format!(
            ".{MIGRATION_LEDGER_FILE}.{}.tmp",
            uuid::Uuid::new_v4().simple()
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| format!("create migration ledger temporary: {error}"))?;
        if let Err(error) = file.write_all(&body).and_then(|()| file.sync_all()) {
            let _ = fs::remove_file(&temporary);
            return Err(format!("persist migration ledger: {error}"));
        }
        if let Err(error) = fs::rename(&temporary, &destination) {
            let _ = fs::remove_file(&temporary);
            return Err(format!("commit migration ledger: {error}"));
        }
        fs::File::open(&self.root)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("sync migration state root: {error}"))
    }
}

fn wall_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Source-side drain of `event/compute/migrate-committed`. Advances
/// `cursor` past every message (same at-least-once semantics as the
/// other drains) and returns the parsed events; the run loop correlates
/// them to its own pending commits by `request_ulid`.
fn drain_committed_events(
    persist: &Persist,
    cursor: &mut Option<String>,
    authorizer: &ActionAuthorizer,
) -> Vec<MigrateCommittedEvent> {
    let msgs = match persist.list_since(MIGRATE_COMMITTED_TOPIC, cursor.as_deref()) {
        Ok(m) => m,
        Err(e) => {
            tracing::debug!(error = %e, "compute_migrate: migrate-committed list_since failed");
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        let body = msg.body.as_deref().unwrap_or("");
        match parse_migrate_committed_event(body) {
            Ok(ev) => {
                if let Err(error) = authorize_event_body(
                    authorizer,
                    body,
                    COMPUTE_MIGRATE_COMMITTED_AUTH_VERB,
                    &ev.vm_id,
                    &ev.source_peer,
                    &ev.target_peer,
                ) {
                    tracing::warn!(
                        ulid = %msg.ulid,
                        vm_id = %ev.vm_id,
                        %error,
                        "compute_migrate: refused unauthorized migrate-committed event"
                    );
                    continue;
                }
                out.push(ev)
            }
            Err(e) => {
                tracing::warn!(ulid = %msg.ulid, error = %e, "compute_migrate: bad migrate-committed event");
            }
        }
    }
    out
}

/// Source-side drain of `event/compute/migrate-failed`. Advances
/// `cursor` past every message and returns the parsed events; the run
/// loop rolls back any pending commit whose `request_ulid` matches
/// (vdi-vm-5).
fn drain_failed_events(
    persist: &Persist,
    cursor: &mut Option<String>,
    authorizer: &ActionAuthorizer,
) -> Vec<MigrateFailedEvent> {
    let msgs = match persist.list_since(MIGRATE_FAILED_TOPIC, cursor.as_deref()) {
        Ok(m) => m,
        Err(e) => {
            tracing::debug!(error = %e, "compute_migrate: migrate-failed list_since failed");
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        let body = msg.body.as_deref().unwrap_or("");
        match parse_migrate_failed_event(body) {
            Ok(ev) => {
                if let Err(error) = authorize_event_body(
                    authorizer,
                    body,
                    COMPUTE_MIGRATE_FAILED_AUTH_VERB,
                    &ev.vm_id,
                    "",
                    &ev.target_peer,
                ) {
                    tracing::warn!(
                        ulid = %msg.ulid,
                        vm_id = %ev.vm_id,
                        %error,
                        "compute_migrate: refused unauthorized migrate-failed event"
                    );
                    continue;
                }
                out.push(ev)
            }
            Err(e) => {
                tracing::warn!(ulid = %msg.ulid, error = %e, "compute_migrate: bad migrate-failed event");
            }
        }
    }
    out
}

/// Worker handle.
#[cfg(test)]
type BusRootFn = dyn Fn() -> Result<PathBuf, String> + Send + Sync;
#[cfg(test)]
type BusOpenHook = dyn Fn(&Path) + Send + Sync;

#[derive(Debug)]
struct MigrationBusTransaction {
    root: PathBuf,
    identity: BusIndexIdentity,
    persist: Persist,
}

fn migration_bus_identity(root: &Path) -> Result<BusIndexIdentity, String> {
    let metadata = fs::metadata(root.join("index.sqlite"))
        .map_err(|error| format!("inspect migration Bus index: {error}"))?;
    if !metadata.is_file() {
        return Err("migration Bus index is not a regular file".into());
    }
    Ok(BusIndexIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

pub struct ComputeMigrateWorker {
    nebula_interface: String,
    nebula_addr_hint: String,
    poll_interval: Duration,
    commit_timeout: Duration,
    bus_root_override: Option<PathBuf>,
    state_root: PathBuf,
    authorizer: Arc<ActionAuthorizer>,
    migration_client: Arc<dyn MigrationAuthority>,
    /// Dynamic Bus-root seam for startup-race tests.
    #[cfg(test)]
    bus_root_resolver_override: Option<Arc<BusRootFn>>,
    /// Runs after opening the SQLite connection but before path-identity
    /// validation, allowing deterministic replacement-race coverage.
    #[cfg(test)]
    bus_open_hook: Option<Arc<BusOpenHook>>,
    #[cfg(test)]
    bus_write_failures: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    bus_read_failure_at: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    event_signer_override: Option<(CloudArmSigner, i64)>,
}

impl Default for ComputeMigrateWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl ComputeMigrateWorker {
    /// Construct with production defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            nebula_interface: DEFAULT_NEBULA_INTERFACE.into(),
            nebula_addr_hint: String::new(),
            poll_interval: DEFAULT_POLL_INTERVAL,
            commit_timeout: DEFAULT_COMMIT_TIMEOUT,
            bus_root_override: None,
            state_root: PathBuf::from(DEFAULT_MIGRATION_STATE_ROOT),
            authorizer: Arc::new(ActionAuthorizer::production()),
            migration_client: Arc::new(WorkloadMigrationClient),
            #[cfg(test)]
            bus_root_resolver_override: None,
            #[cfg(test)]
            bus_open_hook: None,
            #[cfg(test)]
            bus_write_failures: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            bus_read_failure_at: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            event_signer_override: None,
        }
    }

    /// Override the local peer's Nebula address (skips runtime
    /// detection via `ip addr`).
    #[must_use]
    pub fn with_nebula_addr_hint(mut self, addr: String) -> Self {
        self.nebula_addr_hint = addr;
        self
    }

    /// Override the Bus root directory. Used in tests.
    #[must_use]
    pub fn with_bus_root(mut self, p: PathBuf) -> Self {
        self.bus_root_override = Some(p);
        self
    }

    /// Override the root-only migration recovery ledger. Used in tests.
    #[must_use]
    pub fn with_state_root(mut self, p: PathBuf) -> Self {
        self.state_root = p;
        self
    }

    /// Override the poll cadence. Used in tests.
    #[must_use]
    pub fn with_poll_interval(mut self, d: Duration) -> Self {
        self.poll_interval = d;
        self
    }

    /// Override how long the source waits for a `migrate-committed` ack
    /// before rolling back. Used in tests to drive the commit-timeout
    /// path deterministically (vdi-vm-5).
    #[must_use]
    pub fn with_commit_timeout(mut self, d: Duration) -> Self {
        self.commit_timeout = d;
        self
    }

    /// Inject an isolated verifier and replay ledger for focused tests.  The
    /// production constructor always loads the root-only systemd credential and
    /// fails closed when it is unavailable.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_authorizer(mut self, authorizer: Arc<ActionAuthorizer>) -> Self {
        self.authorizer = authorizer;
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_bus_root_resolver(mut self, resolve: Arc<BusRootFn>) -> Self {
        self.bus_root_resolver_override = Some(resolve);
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_bus_open_hook(mut self, hook: Arc<BusOpenHook>) -> Self {
        self.bus_open_hook = Some(hook);
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_migration_authority(mut self, authority: Arc<dyn MigrationAuthority>) -> Self {
        self.migration_client = authority;
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_event_signer(mut self, signer: CloudArmSigner, now_ms: i64) -> Self {
        self.event_signer_override = Some((signer, now_ms));
        self
    }

    fn bus_roots(&self) -> Result<Vec<PathBuf>, String> {
        if let Some(root) = &self.bus_root_override {
            return Ok(vec![root.clone()]);
        }
        #[cfg(test)]
        if let Some(resolve) = self.bus_root_resolver_override.as_ref() {
            return resolve().map(|root| vec![root]);
        }

        let mut roots = Vec::with_capacity(2);
        if let Some(root) = mde_bus::default_data_dir() {
            roots.push(root);
        }
        let system = PathBuf::from(mde_bus::SYSTEM_BUS_ROOT);
        if !roots.contains(&system) {
            roots.push(system);
        }
        Ok(roots)
    }

    fn open_bus(&self) -> Result<MigrationBusTransaction, String> {
        let mut last_error = None;
        for root in self.bus_roots()? {
            let before = match migration_bus_identity(&root) {
                Ok(identity) => identity,
                Err(error) => {
                    last_error = Some(error);
                    continue;
                }
            };
            let persist = match Persist::open(root.clone()) {
                Ok(persist) => persist,
                Err(error) => {
                    last_error = Some(error.to_string());
                    continue;
                }
            };
            #[cfg(test)]
            if let Some(hook) = &self.bus_open_hook {
                hook(&root);
            }
            if migration_bus_identity(&root).is_ok_and(|after| after == before) {
                return Ok(MigrationBusTransaction {
                    root,
                    identity: before,
                    persist,
                });
            }
            last_error = Some("migration Bus index changed while opening".into());
        }
        Err(last_error.unwrap_or_else(|| "migration Bus root is unresolved".into()))
    }

    fn verify_bus(&self, root: &Path, identity: BusIndexIdentity) -> Result<(), String> {
        if migration_bus_identity(root).is_ok_and(|current| current == identity) {
            Ok(())
        } else {
            Err("migration Bus index changed during transaction".into())
        }
    }

    fn gate_bus_read(&self, lane: &str) -> Result<(), String> {
        #[cfg(test)]
        if self
            .bus_read_failure_at
            .fetch_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |remaining| remaining.checked_sub(1),
            )
            .is_ok_and(|remaining| remaining == 1)
        {
            return Err(format!("injected migration Bus {lane} read failure"));
        }
        Ok(())
    }

    fn publish_body(&self, topic: &str, body: &str) -> Result<BusIndexIdentity, String> {
        let transaction = self.open_bus()?;
        #[cfg(test)]
        if self
            .bus_write_failures
            .fetch_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |remaining| remaining.checked_sub(1),
            )
            .is_ok()
        {
            return Err("injected migration Bus write failure".into());
        }
        publish_migration_body(&transaction.persist, topic, body)?;
        self.verify_bus(&transaction.root, transaction.identity)?;
        Ok(transaction.identity)
    }

    fn build_event_body<T: serde::Serialize>(
        &self,
        event: &T,
        verb: &str,
        target: &str,
    ) -> Result<String, String> {
        #[cfg(test)]
        if let Some((signer, now_ms)) = &self.event_signer_override {
            return build_authorized_migrate_event_body(
                event,
                verb,
                target,
                signer,
                &uuid::Uuid::new_v4().to_string(),
                *now_ms,
            );
        }
        build_production_migrate_event_body(event, verb, target)
    }

    fn build_ready_body(&self, event: &MigrateReadyEvent) -> Result<String, String> {
        let target =
            migration_event_auth_target(&event.vm_id, &event.source_peer, &event.target_peer);
        self.build_event_body(event, COMPUTE_MIGRATE_READY_AUTH_VERB, &target)
    }

    fn build_failed_body(
        &self,
        vm_id: &str,
        target_peer: &str,
        request_ulid: &str,
        error: &str,
    ) -> Result<String, String> {
        let event = MigrateFailedEvent {
            vm_id: vm_id.to_owned(),
            target_peer: target_peer.to_owned(),
            request_ulid: request_ulid.to_owned(),
            error: error.to_owned(),
        };
        let target = migration_event_auth_target(vm_id, "", target_peer);
        self.build_event_body(&event, COMPUTE_MIGRATE_FAILED_AUTH_VERB, &target)
    }

    fn build_committed_body(&self, event: &MigrateReadyEvent) -> Result<String, String> {
        let committed = build_migrate_committed_event(event);
        let target = migration_event_auth_target(
            &committed.vm_id,
            &committed.source_peer,
            &committed.target_peer,
        );
        self.build_event_body(&committed, COMPUTE_MIGRATE_COMMITTED_AUTH_VERB, &target)
    }
}

fn resolve_nebula_addr(worker: &ComputeMigrateWorker) -> String {
    if !worker.nebula_addr_hint.is_empty() {
        return worker.nebula_addr_hint.clone();
    }
    local_nebula_addr(&worker.nebula_interface)
}

/// Drain the new source-side migrate requests since `cursor`, advancing the
/// cursor past every message (source or not — same at-least-once semantics as
/// before) and returning the `(request_ulid, request)` pairs this peer is the
/// SOURCE for. Pure Bus I/O — no shell-out — so the heavy [`run_migration`]
/// (which polls virsh for up to [`DEFAULT_SHUTDOWN_TIMEOUT`] and rsyncs a
/// multi-GiB disk) runs on `spawn_blocking` in the run loop instead of inline on
/// the async runtime, and `Persist` (which is `!Sync`) never crosses an `.await`
/// (mackesd-02).
fn drain_source_jobs(
    persist: &Persist,
    worker: &ComputeMigrateWorker,
    cursor: &mut Option<String>,
) -> Vec<(String, MigrateRequest)> {
    let msgs = match persist.list_since(ACTION_TOPIC, cursor.as_deref()) {
        Ok(m) => m,
        Err(e) => {
            tracing::debug!(error = %e, "compute_migrate: list_since failed");
            return Vec::new();
        }
    };
    let own_ip = resolve_nebula_addr(worker);
    let mut jobs = Vec::new();
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        let body = msg.body.as_deref().unwrap_or("");
        let req = match parse_migrate_request(body) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(ulid = %msg.ulid, error = %e, "compute_migrate: bad request");
                continue;
            }
        };
        if !is_source_peer(&req, &own_ip) {
            tracing::debug!(
                ulid = %msg.ulid,
                source = %req.source_peer,
                own = %own_ip,
                "compute_migrate: not source peer; skipping"
            );
            continue;
        }
        if let Err(error) = authorize_source_request(&worker.authorizer, body, &req) {
            tracing::warn!(
                ulid = %msg.ulid,
                vm_id = %req.vm_id,
                %error,
                "compute_migrate: refused unauthorized source request"
            );
            continue;
        }
        jobs.push((msg.ulid.clone(), req));
    }
    jobs
}

/// VIRT-8.b — target-side drain: read `event/compute/migrate-ready`, advance the
/// cursor past every message, and return the events addressed to this peer
/// (`target_peer == own`). The heavy define/start (`run_migrate_target`) then
/// runs on `spawn_blocking` in the run loop (mackesd-02), keeping `Persist` off
/// the `.await`.
fn drain_target_jobs(
    persist: &Persist,
    worker: &ComputeMigrateWorker,
    cursor: &mut Option<String>,
) -> Vec<MigrateReadyEvent> {
    let msgs = match persist.list_since(MIGRATE_READY_TOPIC, cursor.as_deref()) {
        Ok(m) => m,
        Err(e) => {
            tracing::debug!(error = %e, "compute_migrate: migrate-ready list_since failed");
            return Vec::new();
        }
    };
    let own_ip = resolve_nebula_addr(worker);
    let mut jobs = Vec::new();
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        let body = msg.body.as_deref().unwrap_or("");
        let event = match parse_migrate_ready_event(body) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(ulid = %msg.ulid, error = %e, "compute_migrate: bad migrate-ready event");
                continue;
            }
        };
        if !is_target_peer(&event, &own_ip) {
            continue;
        }
        if let Err(error) = authorize_event_body(
            &worker.authorizer,
            body,
            COMPUTE_MIGRATE_READY_AUTH_VERB,
            &event.vm_id,
            &event.source_peer,
            &event.target_peer,
        ) {
            tracing::warn!(
                ulid = %msg.ulid,
                vm_id = %event.vm_id,
                %error,
                "compute_migrate: refused unauthorized migrate-ready event"
            );
            continue;
        }
        jobs.push(event);
    }
    jobs
}

fn verify_source_body(
    authorizer: &ActionAuthorizer,
    body: &str,
    request: &MigrateRequest,
) -> Result<(), String> {
    let target = migration_auth_target(request);
    authorizer
        .verify_exact_body(
            body,
            MutationContext {
                verb: COMPUTE_MIGRATE_AUTH_VERB,
                node: COMPUTE_MIGRATE_NODE_SCOPE,
                target: &target,
            },
        )
        .map(|_| ())
}

fn verify_event_body(
    authorizer: &ActionAuthorizer,
    body: &str,
    verb: &str,
    vm_id: &str,
    source_peer: &str,
    target_peer: &str,
) -> Result<(), String> {
    let target = migration_event_auth_target(vm_id, source_peer, target_peer);
    authorizer
        .verify_exact_body(
            body,
            MutationContext {
                verb,
                node: COMPUTE_MIGRATE_NODE_SCOPE,
                target: &target,
            },
        )
        .map(|_| ())
}

fn replay_owned_during_recovery(
    result: Result<(), String>,
    recovering: bool,
) -> Result<(), String> {
    match result {
        Err(error) if recovering && error == "armed token was already used" => Ok(()),
        other => other,
    }
}

fn recover_prepared_jobs(
    ledger: &mut MigrationLedger,
    authorizer: &ActionAuthorizer,
) -> Result<(), String> {
    for job in &mut ledger.state.source_jobs {
        if job.authorized && job.effect_claimed && job.failure.is_none() {
            job.failure = Some(DurableSourceFailure {
                error: "source migration effect outcome indeterminate after restart; effect was not repeated"
                    .into(),
                body: None,
            });
        }
    }
    for index in 0..ledger.state.source_jobs.len() {
        if ledger.state.source_jobs[index].authorized {
            continue;
        }
        let job = ledger.state.source_jobs[index].clone();
        let accepted = replay_owned_during_recovery(
            authorize_source_request(authorizer, &job.raw_body, &job.request),
            true,
        )
        .is_ok();
        ledger.state.source_jobs[index].authorized = accepted;
    }
    ledger.state.source_jobs.retain(|job| job.authorized);

    for index in 0..ledger.state.target_jobs.len() {
        if matches!(
            ledger.state.target_jobs[index].phase,
            TargetJobPhase::Applying
        ) {
            ledger.state.target_jobs[index].phase = TargetJobPhase::PublishFailed {
                error: "target migration effect outcome indeterminate after restart; effect was not repeated"
                    .into(),
            };
        }
        if !matches!(
            ledger.state.target_jobs[index].phase,
            TargetJobPhase::Prepared
        ) {
            continue;
        }
        let job = ledger.state.target_jobs[index].clone();
        let accepted = replay_owned_during_recovery(
            authorize_event_body(
                authorizer,
                &job.raw_body,
                COMPUTE_MIGRATE_READY_AUTH_VERB,
                &job.event.vm_id,
                &job.event.source_peer,
                &job.event.target_peer,
            ),
            true,
        )
        .is_ok();
        if accepted {
            ledger.state.target_jobs[index].phase = TargetJobPhase::Apply;
        }
    }
    ledger
        .state
        .target_jobs
        .retain(|job| !matches!(job.phase, TargetJobPhase::Prepared));

    for index in 0..ledger.state.ack_jobs.len() {
        if ledger.state.ack_jobs[index].authorized {
            continue;
        }
        let job = ledger.state.ack_jobs[index].clone();
        let result = match &job.event {
            DurableAckEvent::Committed { event } => authorize_event_body(
                authorizer,
                &job.raw_body,
                COMPUTE_MIGRATE_COMMITTED_AUTH_VERB,
                &event.vm_id,
                &event.source_peer,
                &event.target_peer,
            ),
            DurableAckEvent::Failed { event } => authorize_event_body(
                authorizer,
                &job.raw_body,
                COMPUTE_MIGRATE_FAILED_AUTH_VERB,
                &event.vm_id,
                "",
                &event.target_peer,
            ),
        };
        ledger.state.ack_jobs[index].authorized =
            replay_owned_during_recovery(result, true).is_ok();
    }
    ledger.state.ack_jobs.retain(|job| job.authorized);
    for pending in &mut ledger.state.pending_commits {
        pending.phase = match &pending.phase {
            PendingPhase::Relinquishing => PendingPhase::Indeterminate {
                operation: "relinquish".into(),
                reason:
                    "source relinquish outcome indeterminate after restart; effect was not repeated"
                        .into(),
            },
            PendingPhase::RollingBack { .. } => PendingPhase::Indeterminate {
                operation: "rollback".into(),
                reason:
                    "source rollback outcome indeterminate after restart; effect was not repeated"
                        .into(),
            },
            phase => phase.clone(),
        };
    }
    ledger.store()
}

struct MigrationBusSweep {
    identity: BusIndexIdentity,
    activating: bool,
    source_tail: Option<String>,
    target_tail: Option<String>,
    source: Vec<StoredMessage>,
    target: Vec<StoredMessage>,
    committed: Vec<StoredMessage>,
    failed: Vec<StoredMessage>,
}

fn bounded_lane(messages: Vec<StoredMessage>, lane: &str) -> Result<Vec<StoredMessage>, String> {
    if messages.len() > MAX_MIGRATION_JOBS {
        Err(format!(
            "migration Bus {lane} lane exceeds its bounded sweep"
        ))
    } else {
        Ok(messages)
    }
}

fn stage_bus_sweep(
    transaction: &MigrationBusTransaction,
    worker: &ComputeMigrateWorker,
    ledger: &MigrationLedger,
) -> Result<MigrationBusSweep, String> {
    let activating = ledger.state.bus_identity != Some(transaction.identity);
    worker.gate_bus_read("source")?;
    let (source_tail, target_tail, source, target) = if activating {
        let source_tail = transaction
            .persist
            .latest_ulid(ACTION_TOPIC)
            .map_err(|error| format!("tail source migration actions: {error}"))?;
        worker.gate_bus_read("target")?;
        let target_tail = transaction
            .persist
            .latest_ulid(MIGRATE_READY_TOPIC)
            .map_err(|error| format!("tail target migration events: {error}"))?;
        (source_tail, target_tail, Vec::new(), Vec::new())
    } else {
        let source = bounded_lane(
            transaction
                .persist
                .list_since(ACTION_TOPIC, ledger.state.source_cursor.as_deref())
                .map_err(|error| format!("list source migration actions: {error}"))?,
            "source",
        )?;
        worker.gate_bus_read("target")?;
        let target = bounded_lane(
            transaction
                .persist
                .list_since(MIGRATE_READY_TOPIC, ledger.state.target_cursor.as_deref())
                .map_err(|error| format!("list target migration events: {error}"))?,
            "target",
        )?;
        (
            ledger.state.source_cursor.clone(),
            ledger.state.target_cursor.clone(),
            source,
            target,
        )
    };
    // Acknowledgements are durable replies. On a replacement index they fold
    // from the beginning so an outstanding source transaction can still
    // converge; source/target commands above are transient and tail-activate.
    let committed_cursor = if activating {
        None
    } else {
        ledger.state.committed_cursor.as_deref()
    };
    let failed_cursor = if activating {
        None
    } else {
        ledger.state.failed_cursor.as_deref()
    };
    worker.gate_bus_read("committed")?;
    let committed = bounded_lane(
        transaction
            .persist
            .list_since(MIGRATE_COMMITTED_TOPIC, committed_cursor)
            .map_err(|error| format!("list committed migration events: {error}"))?,
        "committed",
    )?;
    worker.gate_bus_read("failed")?;
    let failed = bounded_lane(
        transaction
            .persist
            .list_since(MIGRATE_FAILED_TOPIC, failed_cursor)
            .map_err(|error| format!("list failed migration events: {error}"))?,
        "failed",
    )?;
    worker.verify_bus(&transaction.root, transaction.identity)?;
    Ok(MigrationBusSweep {
        identity: transaction.identity,
        activating,
        source_tail,
        target_tail,
        source,
        target,
        committed,
        failed,
    })
}

fn admit_source_messages(
    messages: Vec<StoredMessage>,
    worker: &ComputeMigrateWorker,
    ledger: &mut MigrationLedger,
) -> Result<(), String> {
    let own_ip = resolve_nebula_addr(worker);
    for message in messages {
        ledger.state.source_cursor = Some(message.ulid.clone());
        let body = message.body.as_deref().unwrap_or("");
        let request = parse_migrate_request(body).ok();
        let relevant = request
            .as_ref()
            .is_some_and(|request| is_source_peer(request, &own_ip));
        if !relevant {
            ledger.store()?;
            continue;
        }
        let request = request.expect("relevant migration request");
        if verify_source_body(worker.authorizer.as_ref(), body, &request).is_err()
            || ledger.state.source_jobs.len() >= MAX_MIGRATION_JOBS
        {
            ledger.store()?;
            continue;
        }
        ledger.state.source_jobs.push(DurableSourceJob {
            message_ulid: message.ulid,
            raw_body: body.to_owned(),
            request,
            authorized: false,
            effect_claimed: false,
            failure: None,
        });
        ledger.store()?;
        let index = ledger.state.source_jobs.len() - 1;
        let job = &ledger.state.source_jobs[index];
        if authorize_source_request(worker.authorizer.as_ref(), &job.raw_body, &job.request).is_ok()
        {
            ledger.state.source_jobs[index].authorized = true;
        } else {
            ledger.state.source_jobs.remove(index);
        }
        ledger.store()?;
    }
    Ok(())
}

fn admit_source_jobs(
    persist: &Persist,
    worker: &ComputeMigrateWorker,
    ledger: &mut MigrationLedger,
) -> Result<(), String> {
    let messages = persist
        .list_since(ACTION_TOPIC, ledger.state.source_cursor.as_deref())
        .map_err(|error| format!("list source migration actions: {error}"))?;
    admit_source_messages(messages, worker, ledger)
}

fn admit_target_messages(
    messages: Vec<StoredMessage>,
    worker: &ComputeMigrateWorker,
    ledger: &mut MigrationLedger,
) -> Result<(), String> {
    let own_ip = resolve_nebula_addr(worker);
    for message in messages {
        ledger.state.target_cursor = Some(message.ulid.clone());
        let body = message.body.as_deref().unwrap_or("");
        let event = parse_migrate_ready_event(body).ok();
        let relevant = event
            .as_ref()
            .is_some_and(|event| is_target_peer(event, &own_ip));
        if !relevant {
            ledger.store()?;
            continue;
        }
        let event = event.expect("relevant migration-ready event");
        if verify_event_body(
            worker.authorizer.as_ref(),
            body,
            COMPUTE_MIGRATE_READY_AUTH_VERB,
            &event.vm_id,
            &event.source_peer,
            &event.target_peer,
        )
        .is_err()
            || ledger.state.target_jobs.len() >= MAX_MIGRATION_JOBS
        {
            ledger.store()?;
            continue;
        }
        ledger.state.target_jobs.push(DurableTargetJob {
            message_ulid: message.ulid,
            raw_body: body.to_owned(),
            event,
            phase: TargetJobPhase::Prepared,
            reply_body: None,
        });
        ledger.store()?;
        let index = ledger.state.target_jobs.len() - 1;
        let job = &ledger.state.target_jobs[index];
        if authorize_event_body(
            worker.authorizer.as_ref(),
            &job.raw_body,
            COMPUTE_MIGRATE_READY_AUTH_VERB,
            &job.event.vm_id,
            &job.event.source_peer,
            &job.event.target_peer,
        )
        .is_ok()
        {
            ledger.state.target_jobs[index].phase = TargetJobPhase::Apply;
        } else {
            ledger.state.target_jobs.remove(index);
        }
        ledger.store()?;
    }
    Ok(())
}

fn admit_target_jobs(
    persist: &Persist,
    worker: &ComputeMigrateWorker,
    ledger: &mut MigrationLedger,
) -> Result<(), String> {
    let messages = persist
        .list_since(MIGRATE_READY_TOPIC, ledger.state.target_cursor.as_deref())
        .map_err(|error| format!("list target migration events: {error}"))?;
    admit_target_messages(messages, worker, ledger)
}

fn admit_ack_messages(
    committed: Vec<StoredMessage>,
    failed: Vec<StoredMessage>,
    worker: &ComputeMigrateWorker,
    ledger: &mut MigrationLedger,
) -> Result<(), String> {
    for message in committed {
        ledger.state.committed_cursor = Some(message.ulid.clone());
        let body = message.body.as_deref().unwrap_or("");
        let Some(event) = parse_migrate_committed_event(body).ok() else {
            ledger.store()?;
            continue;
        };
        if !ledger
            .state
            .pending_commits
            .iter()
            .any(|pending| pending.request_ulid == event.request_ulid)
            || verify_event_body(
                worker.authorizer.as_ref(),
                body,
                COMPUTE_MIGRATE_COMMITTED_AUTH_VERB,
                &event.vm_id,
                &event.source_peer,
                &event.target_peer,
            )
            .is_err()
            || ledger.state.ack_jobs.len() >= MAX_MIGRATION_JOBS
        {
            ledger.store()?;
            continue;
        }
        ledger.state.ack_jobs.push(DurableAckJob {
            message_ulid: message.ulid,
            raw_body: body.to_owned(),
            event: DurableAckEvent::Committed { event },
            authorized: false,
        });
        ledger.store()?;
        let index = ledger.state.ack_jobs.len() - 1;
        let job = ledger.state.ack_jobs[index].clone();
        let result = match &job.event {
            DurableAckEvent::Committed { event } => authorize_event_body(
                worker.authorizer.as_ref(),
                &job.raw_body,
                COMPUTE_MIGRATE_COMMITTED_AUTH_VERB,
                &event.vm_id,
                &event.source_peer,
                &event.target_peer,
            ),
            DurableAckEvent::Failed { .. } => unreachable!(),
        };
        if result.is_ok() {
            ledger.state.ack_jobs[index].authorized = true;
        } else {
            ledger.state.ack_jobs.remove(index);
        }
        ledger.store()?;
    }

    for message in failed {
        ledger.state.failed_cursor = Some(message.ulid.clone());
        let body = message.body.as_deref().unwrap_or("");
        let Some(event) = parse_migrate_failed_event(body).ok() else {
            ledger.store()?;
            continue;
        };
        if !ledger
            .state
            .pending_commits
            .iter()
            .any(|pending| pending.request_ulid == event.request_ulid)
            || verify_event_body(
                worker.authorizer.as_ref(),
                body,
                COMPUTE_MIGRATE_FAILED_AUTH_VERB,
                &event.vm_id,
                "",
                &event.target_peer,
            )
            .is_err()
            || ledger.state.ack_jobs.len() >= MAX_MIGRATION_JOBS
        {
            ledger.store()?;
            continue;
        }
        ledger.state.ack_jobs.push(DurableAckJob {
            message_ulid: message.ulid,
            raw_body: body.to_owned(),
            event: DurableAckEvent::Failed { event },
            authorized: false,
        });
        ledger.store()?;
        let index = ledger.state.ack_jobs.len() - 1;
        let job = ledger.state.ack_jobs[index].clone();
        let result = match &job.event {
            DurableAckEvent::Failed { event } => authorize_event_body(
                worker.authorizer.as_ref(),
                &job.raw_body,
                COMPUTE_MIGRATE_FAILED_AUTH_VERB,
                &event.vm_id,
                "",
                &event.target_peer,
            ),
            DurableAckEvent::Committed { .. } => unreachable!(),
        };
        if result.is_ok() {
            ledger.state.ack_jobs[index].authorized = true;
        } else {
            ledger.state.ack_jobs.remove(index);
        }
        ledger.store()?;
    }
    Ok(())
}

fn admit_ack_jobs(
    persist: &Persist,
    worker: &ComputeMigrateWorker,
    ledger: &mut MigrationLedger,
) -> Result<(), String> {
    let committed = persist
        .list_since(
            MIGRATE_COMMITTED_TOPIC,
            ledger.state.committed_cursor.as_deref(),
        )
        .map_err(|error| format!("list committed migration events: {error}"))?;
    let failed = persist
        .list_since(MIGRATE_FAILED_TOPIC, ledger.state.failed_cursor.as_deref())
        .map_err(|error| format!("list failed migration events: {error}"))?;
    admit_ack_messages(committed, failed, worker, ledger)
}

fn apply_ack_jobs(ledger: &mut MigrationLedger, now_ms: i64) -> Result<(), String> {
    for ack in ledger.state.ack_jobs.drain(..) {
        if !ack.authorized {
            continue;
        }
        match ack.event {
            DurableAckEvent::Committed { event } => {
                if let Some(pending) = ledger
                    .state
                    .pending_commits
                    .iter_mut()
                    .find(|pending| pending.request_ulid == event.request_ulid)
                {
                    pending.phase = PendingPhase::Relinquish;
                    pending.next_attempt_ms = now_ms;
                }
            }
            DurableAckEvent::Failed { event } => {
                if let Some(pending) = ledger
                    .state
                    .pending_commits
                    .iter_mut()
                    .find(|pending| pending.request_ulid == event.request_ulid)
                {
                    pending.phase = PendingPhase::Rollback {
                        reason: format!("target failed: {}", event.error),
                    };
                    pending.next_attempt_ms = now_ms;
                }
            }
        }
    }
    ledger.store()
}

/// Resolve the shared Bus spool. System workers do not necessarily have HOME
/// or XDG state during early boot, so the canonical `/run/mde-bus` root is the
/// final authority when the environment resolver is unavailable.
fn compute_migrate_bus_root(override_root: Option<PathBuf>) -> PathBuf {
    compute_migrate_bus_root_or_system(override_root.or_else(mde_bus::default_data_dir))
}

fn compute_migrate_bus_root_or_system(resolved: Option<PathBuf>) -> PathBuf {
    resolved.unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
}

/// Admit one coherent Bus sweep before any migration effect is allowed. Each
/// admission checkpoints its cursor and prepared record atomically in the
/// migration ledger. If any lane is unreadable, the caller performs no source
/// migration, target apply, commit relinquish, timeout rollback, or retry.
/// Partial admissions are safe: their durable cursors prevent duplication on
/// the next successful sweep.
fn admit_bus_jobs(
    persist: &Persist,
    worker: &ComputeMigrateWorker,
    ledger: &mut MigrationLedger,
) -> Result<(), String> {
    admit_source_jobs(persist, worker, ledger)?;
    admit_target_jobs(persist, worker, ledger)?;
    admit_ack_jobs(persist, worker, ledger)
}

fn commit_bus_sweep(
    sweep: MigrationBusSweep,
    worker: &ComputeMigrateWorker,
    ledger: &mut MigrationLedger,
) -> Result<(), String> {
    if sweep.activating {
        ledger.state.bus_identity = Some(sweep.identity);
        ledger.state.source_cursor = sweep.source_tail;
        ledger.state.target_cursor = sweep.target_tail;
        ledger.state.committed_cursor = None;
        ledger.state.failed_cursor = None;
        ledger.store()?;
    }
    admit_source_messages(sweep.source, worker, ledger)?;
    admit_target_messages(sweep.target, worker, ledger)?;
    admit_ack_messages(sweep.committed, sweep.failed, worker, ledger)
}

impl ComputeMigrateWorker {
    fn require_current_bus(&self, expected: BusIndexIdentity) -> Result<(), String> {
        let transaction = self.open_bus()?;
        if transaction.identity != expected {
            return Err("migration Bus changed before effects".into());
        }
        self.verify_bus(&transaction.root, transaction.identity)
    }

    async fn cycle(&self, ledger: &mut MigrationLedger) -> Result<(), String> {
        let transaction = self.open_bus()?;
        let sweep = stage_bus_sweep(&transaction, self, ledger)?;
        let read_identity = sweep.identity;
        drop(transaction);
        commit_bus_sweep(sweep, self, ledger)?;
        self.require_current_bus(read_identity)?;

        let source_ids = ledger
            .state
            .source_jobs
            .iter()
            .filter(|job| job.authorized && !job.effect_claimed && job.failure.is_none())
            .map(|job| job.message_ulid.clone())
            .collect::<Vec<_>>();
        for id in source_ids {
            let Some(index) = ledger
                .state
                .source_jobs
                .iter()
                .position(|job| job.message_ulid == id)
            else {
                continue;
            };
            self.require_current_bus(read_identity)?;
            ledger.state.source_jobs[index].effect_claimed = true;
            ledger.store()?;
            let job = ledger.state.source_jobs[index].clone();
            let request = job.request.clone();
            let client = Arc::clone(&self.migration_client);
            let result =
                tokio::task::spawn_blocking(move || run_migration(&request, client.as_ref())).await;
            let Some(index) = ledger
                .state
                .source_jobs
                .iter()
                .position(|queued| queued.message_ulid == id)
            else {
                continue;
            };
            match result {
                Ok(MigrationOutcome::Ok { domain_xml }) => {
                    let event = build_migrate_ready_event(
                        &job.request,
                        target_disk_path_for(&job.request.disk_path, DEFAULT_TARGET_VM_DIR),
                        job.message_ulid.clone(),
                        domain_xml.clone(),
                    );
                    let now_ms = wall_now_ms();
                    let timeout_ms =
                        i64::try_from(self.commit_timeout.as_millis()).unwrap_or(i64::MAX);
                    ledger.state.pending_commits.push(PendingCommit {
                        request_ulid: job.message_ulid.clone(),
                        vm_id: job.request.vm_id.clone(),
                        domain_xml,
                        ready_event: event,
                        ready_body: None,
                        deadline_ms: now_ms.saturating_add(timeout_ms),
                        phase: PendingPhase::PublishReady,
                        next_attempt_ms: now_ms,
                    });
                    ledger.state.source_jobs.remove(index);
                }
                Ok(outcome) => {
                    ledger.state.source_jobs[index].failure = Some(DurableSourceFailure {
                        error: format!("source migration failed: {outcome:?}"),
                        body: None,
                    });
                }
                Err(error) => {
                    ledger.state.source_jobs[index].failure = Some(DurableSourceFailure {
                        error: format!("source migration task outcome indeterminate: {error}"),
                        body: None,
                    });
                }
            }
            ledger.store()?;
        }

        let failed_source_ids = ledger
            .state
            .source_jobs
            .iter()
            .filter(|job| job.failure.is_some())
            .map(|job| job.message_ulid.clone())
            .collect::<Vec<_>>();
        for id in failed_source_ids {
            let index = ledger
                .state
                .source_jobs
                .iter()
                .position(|job| job.message_ulid == id)
                .ok_or_else(|| "source failure outbox disappeared".to_string())?;
            if ledger.state.source_jobs[index]
                .failure
                .as_ref()
                .and_then(|failure| failure.body.as_ref())
                .is_none()
            {
                let job = &ledger.state.source_jobs[index];
                let failure = job.failure.as_ref().expect("filtered source failure");
                let body = self.build_failed_body(
                    &job.request.vm_id,
                    &job.request.target_peer,
                    &job.message_ulid,
                    &failure.error,
                )?;
                ledger.state.source_jobs[index]
                    .failure
                    .as_mut()
                    .expect("filtered source failure")
                    .body = Some(body);
                ledger.store()?;
            }
            let body = ledger.state.source_jobs[index]
                .failure
                .as_ref()
                .and_then(|failure| failure.body.clone())
                .ok_or_else(|| "source failure outbox body missing".to_string())?;
            self.publish_body(MIGRATE_FAILED_TOPIC, &body)?;
            ledger.state.source_jobs.remove(index);
            ledger.store()?;
        }

        let now_ms = wall_now_ms();
        let ready_ids = ledger
            .state
            .pending_commits
            .iter()
            .filter(|pending| {
                matches!(pending.phase, PendingPhase::PublishReady)
                    && now_ms >= pending.next_attempt_ms
            })
            .map(|pending| pending.request_ulid.clone())
            .collect::<Vec<_>>();
        for id in ready_ids {
            let index = ledger
                .state
                .pending_commits
                .iter()
                .position(|pending| pending.request_ulid == id)
                .ok_or_else(|| "ready outbox disappeared".to_string())?;
            if ledger.state.pending_commits[index].ready_body.is_none() {
                let body =
                    self.build_ready_body(&ledger.state.pending_commits[index].ready_event)?;
                ledger.state.pending_commits[index].ready_body = Some(body);
                ledger.store()?;
            }
            let body = ledger.state.pending_commits[index]
                .ready_body
                .clone()
                .ok_or_else(|| "ready outbox body missing".to_string())?;
            self.publish_body(MIGRATE_READY_TOPIC, &body)?;
            ledger.state.pending_commits[index].phase = PendingPhase::Waiting;
            ledger.store()?;
        }

        let target_ids = ledger
            .state
            .target_jobs
            .iter()
            .map(|job| job.message_ulid.clone())
            .collect::<Vec<_>>();
        for id in target_ids {
            let Some(index) = ledger
                .state
                .target_jobs
                .iter()
                .position(|job| job.message_ulid == id)
            else {
                continue;
            };
            if matches!(ledger.state.target_jobs[index].phase, TargetJobPhase::Apply) {
                self.require_current_bus(read_identity)?;
                ledger.state.target_jobs[index].phase = TargetJobPhase::Applying;
                ledger.store()?;
                let event = ledger.state.target_jobs[index].event.clone();
                let event_run = event.clone();
                let client = Arc::clone(&self.migration_client);
                let result = tokio::task::spawn_blocking(move || {
                    run_migrate_target(&event_run, client.as_ref())
                })
                .await;
                let index = ledger
                    .state
                    .target_jobs
                    .iter()
                    .position(|job| job.message_ulid == id)
                    .ok_or_else(|| "claimed target migration disappeared".to_string())?;
                ledger.state.target_jobs[index].phase = match result {
                    Ok(Ok(())) => TargetJobPhase::PublishCommitted,
                    Ok(Err(error)) => TargetJobPhase::PublishFailed { error },
                    Err(error) => TargetJobPhase::PublishFailed {
                        error: format!(
                            "target migration task outcome indeterminate: {error}; effect was not repeated"
                        ),
                    },
                };
                ledger.store()?;
            }
            let index = ledger
                .state
                .target_jobs
                .iter()
                .position(|job| job.message_ulid == id)
                .ok_or_else(|| "target outbox disappeared".to_string())?;
            let phase = ledger.state.target_jobs[index].phase.clone();
            if matches!(phase, TargetJobPhase::Applying | TargetJobPhase::Prepared) {
                continue;
            }
            if ledger.state.target_jobs[index].reply_body.is_none() {
                let body = match &phase {
                    TargetJobPhase::PublishCommitted => {
                        self.build_committed_body(&ledger.state.target_jobs[index].event)?
                    }
                    TargetJobPhase::PublishFailed { error } => {
                        let event = &ledger.state.target_jobs[index].event;
                        self.build_failed_body(
                            &event.vm_id,
                            &event.target_peer,
                            &event.request_ulid,
                            error,
                        )?
                    }
                    _ => continue,
                };
                ledger.state.target_jobs[index].reply_body = Some(body);
                ledger.store()?;
            }
            let topic = if matches!(phase, TargetJobPhase::PublishCommitted) {
                MIGRATE_COMMITTED_TOPIC
            } else {
                MIGRATE_FAILED_TOPIC
            };
            let body = ledger.state.target_jobs[index]
                .reply_body
                .clone()
                .ok_or_else(|| "target reply outbox body missing".to_string())?;
            self.publish_body(topic, &body)?;
            ledger.state.target_jobs.remove(index);
            ledger.store()?;
        }

        apply_ack_jobs(ledger, now_ms)?;
        for pending in &mut ledger.state.pending_commits {
            if matches!(pending.phase, PendingPhase::Waiting) && now_ms >= pending.deadline_ms {
                pending.phase = PendingPhase::Rollback {
                    reason: "commit timeout".into(),
                };
                pending.next_attempt_ms = now_ms;
            }
        }
        ledger.store()?;

        let terminal_ids = ledger
            .state
            .pending_commits
            .iter()
            .filter(|pending| {
                now_ms >= pending.next_attempt_ms
                    && matches!(
                        pending.phase,
                        PendingPhase::Relinquish | PendingPhase::Rollback { .. }
                    )
            })
            .map(|pending| pending.request_ulid.clone())
            .collect::<Vec<_>>();
        for id in terminal_ids {
            self.require_current_bus(read_identity)?;
            let index = ledger
                .state
                .pending_commits
                .iter()
                .position(|pending| pending.request_ulid == id)
                .ok_or_else(|| "terminal migration disappeared".to_string())?;
            let pending = ledger.state.pending_commits[index].clone();
            ledger.state.pending_commits[index].phase = match &pending.phase {
                PendingPhase::Relinquish => PendingPhase::Relinquishing,
                PendingPhase::Rollback { reason } => PendingPhase::RollingBack {
                    reason: reason.clone(),
                },
                _ => continue,
            };
            ledger.store()?;
            let client = Arc::clone(&self.migration_client);
            let result = match &pending.phase {
                PendingPhase::Relinquish => {
                    let vm = pending.vm_id.clone();
                    tokio::task::spawn_blocking(move || run_source_undefine(&vm, client.as_ref()))
                        .await
                }
                PendingPhase::Rollback { .. } => {
                    let vm = pending.vm_id.clone();
                    let xml = pending.domain_xml.clone();
                    tokio::task::spawn_blocking(move || {
                        run_source_rollback(&vm, &xml, client.as_ref())
                    })
                    .await
                }
                _ => continue,
            };
            let index = ledger
                .state
                .pending_commits
                .iter()
                .position(|queued| queued.request_ulid == id)
                .ok_or_else(|| "claimed terminal migration disappeared".to_string())?;
            match result {
                Ok(Ok(())) => {
                    ledger.state.pending_commits.remove(index);
                }
                Ok(Err(error)) => {
                    let operation = match pending.phase {
                        PendingPhase::Relinquish => "relinquish",
                        PendingPhase::Rollback { .. } => "rollback",
                        _ => unreachable!("terminal work was filtered before claim"),
                    };
                    ledger.state.pending_commits[index].phase = PendingPhase::Indeterminate {
                        operation: operation.into(),
                        reason: format!(
                            "source {operation} returned an error after its durable effect claim: {error}; effect was not repeated"
                        ),
                    };
                }
                Err(error) => {
                    let operation = match pending.phase {
                        PendingPhase::Relinquish => "relinquish",
                        PendingPhase::Rollback { .. } => "rollback",
                        _ => unreachable!("terminal work was filtered before claim"),
                    };
                    ledger.state.pending_commits[index].phase = PendingPhase::Indeterminate {
                        operation: operation.into(),
                        reason: format!(
                            "source {operation} task outcome indeterminate after its durable effect claim: {error}; effect was not repeated"
                        ),
                    };
                }
            }
            ledger.store()?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl Worker for ComputeMigrateWorker {
    fn name(&self) -> &'static str {
        "compute_migrate"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let retry_interval = self
            .poll_interval
            .clamp(MIN_BUS_RETRY_INTERVAL, MAX_BUS_RETRY_INTERVAL);
        loop {
            match self.open_bus() {
                Ok(transaction) => {
                    drop(transaction);
                    break;
                }
                Err(error) => tracing::warn!(
                    %error,
                    "compute_migrate: Bus open failed; startup will retry"
                ),
            }
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(retry_interval) => {}
            }
        }

        let mut ledger = MigrationLedger::open(&self.state_root)
            .map_err(|error| anyhow::anyhow!("compute migration recovery unavailable: {error}"))?;
        recover_prepared_jobs(&mut ledger, self.authorizer.as_ref())
            .map_err(|error| anyhow::anyhow!("recover prepared migrations: {error}"))?;
        let mut tick = tokio::time::interval(self.poll_interval);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if let Err(error) = self.cycle(&mut ledger).await {
                        tracing::warn!(
                            %error,
                            "compute_migrate: transaction deferred; durable work retained"
                        );
                    }
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
    use crate::ipc::action_auth::{authorize_test_body, ActionAuthorizer};
    use mackes_mesh_types::cloud::CloudArmSigner;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct FakeMigrationActuator {
        calls: Mutex<Vec<String>>,
        stop_error: bool,
        rollback_error: bool,
        relinquish_error: bool,
    }

    impl FakeMigrationActuator {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().expect("fake calls").clone()
        }

        fn record(&self, call: impl Into<String>) {
            self.calls.lock().expect("fake calls").push(call.into());
        }
    }

    impl MigrationAuthority for FakeMigrationActuator {
        fn capture_definition(&self, vm_id: &str) -> Result<String, WorkloadActuatorError> {
            self.record(format!("capture:{vm_id}"));
            Ok(format!("<domain><name>{vm_id}</name></domain>"))
        }

        fn request_stop(&self, vm_id: &str) -> Result<(), WorkloadActuatorError> {
            self.record(format!("stop:{vm_id}"));
            if self.stop_error {
                Err(WorkloadActuatorError::Retryable(
                    "hostile stop refusal".into(),
                ))
            } else {
                Ok(())
            }
        }

        fn is_stopped(&self, vm_id: &str) -> Result<bool, WorkloadActuatorError> {
            self.record(format!("observe:{vm_id}"));
            Ok(true)
        }

        fn define_and_start(
            &self,
            vm_id: &str,
            domain_xml: &str,
        ) -> Result<(), WorkloadActuatorError> {
            self.record(format!("define-start:{vm_id}"));
            if self.rollback_error {
                Err(WorkloadActuatorError::Retryable(
                    "hostile rollback error after partial effect".into(),
                ))
            } else if domain_xml.trim().is_empty() {
                Err(WorkloadActuatorError::Permanent(
                    "migration definition is empty".into(),
                ))
            } else {
                Ok(())
            }
        }

        fn relinquish_definition(&self, vm_id: &str) -> Result<(), WorkloadActuatorError> {
            self.record(format!("relinquish:{vm_id}"));
            if self.relinquish_error {
                Err(WorkloadActuatorError::Retryable(
                    "hostile relinquish error after partial effect".into(),
                ))
            } else {
                Ok(())
            }
        }
    }

    const AUTH_KEY: &[u8] = b"compute-migrate-test-key";
    const AUTH_NOW_MS: i64 = 1_700_000_000_000;

    fn request_body(req: &MigrateRequest) -> String {
        let mut value = serde_json::to_value(req).expect("request value");
        value["schema_version"] = serde_json::Value::from(1_u64);
        value.to_string()
    }

    fn authorized_request_body(req: &MigrateRequest, nonce: &str) -> String {
        let unsigned = request_body(req);
        let target = migration_auth_target(req);
        authorize_test_body(
            AUTH_KEY,
            &unsigned,
            MutationContext {
                verb: COMPUTE_MIGRATE_AUTH_VERB,
                node: COMPUTE_MIGRATE_NODE_SCOPE,
                target: &target,
            },
            nonce,
            AUTH_NOW_MS + 30_000,
        )
    }

    fn test_worker_at(auth_root: &std::path::Path, own_ip: &str) -> ComputeMigrateWorker {
        ComputeMigrateWorker::new()
            .with_nebula_addr_hint(own_ip.into())
            .with_event_signer(
                CloudArmSigner::new(AUTH_KEY.to_vec()).expect("test signer"),
                AUTH_NOW_MS,
            )
            .with_authorizer(Arc::new(ActionAuthorizer::for_test(
                AUTH_KEY,
                auth_root.to_path_buf(),
                AUTH_NOW_MS,
            )))
    }

    fn test_worker(auth_root: &std::path::Path) -> ComputeMigrateWorker {
        test_worker_at(auth_root, "10.42.0.1")
    }

    fn authorized_ready_body(event: &MigrateReadyEvent, nonce: &str) -> String {
        let mut value = serde_json::to_value(event).expect("ready value");
        value["schema_version"] = serde_json::Value::from(1_u64);
        let unsigned = value.to_string();
        let target =
            migration_event_auth_target(&event.vm_id, &event.source_peer, &event.target_peer);
        authorize_test_body(
            AUTH_KEY,
            &unsigned,
            MutationContext {
                verb: COMPUTE_MIGRATE_READY_AUTH_VERB,
                node: COMPUTE_MIGRATE_NODE_SCOPE,
                target: &target,
            },
            nonce,
            AUTH_NOW_MS + 30_000,
        )
    }

    fn authorized_committed_body(event: &MigrateCommittedEvent, nonce: &str) -> String {
        let mut value = serde_json::to_value(event).expect("committed value");
        value["schema_version"] = serde_json::Value::from(1_u64);
        let unsigned = value.to_string();
        let target =
            migration_event_auth_target(&event.vm_id, &event.source_peer, &event.target_peer);
        authorize_test_body(
            AUTH_KEY,
            &unsigned,
            MutationContext {
                verb: COMPUTE_MIGRATE_COMMITTED_AUTH_VERB,
                node: COMPUTE_MIGRATE_NODE_SCOPE,
                target: &target,
            },
            nonce,
            AUTH_NOW_MS + 30_000,
        )
    }

    fn authorized_failed_body(event: &MigrateFailedEvent, nonce: &str) -> String {
        let mut value = serde_json::to_value(event).expect("failed value");
        value["schema_version"] = serde_json::Value::from(1_u64);
        let unsigned = value.to_string();
        let target = migration_event_auth_target(&event.vm_id, "", &event.target_peer);
        authorize_test_body(
            AUTH_KEY,
            &unsigned,
            MutationContext {
                verb: COMPUTE_MIGRATE_FAILED_AUTH_VERB,
                node: COMPUTE_MIGRATE_NODE_SCOPE,
                target: &target,
            },
            nonce,
            AUTH_NOW_MS + 30_000,
        )
    }

    // ── parse_migrate_request ──

    #[test]
    fn parse_migrate_happy_path() {
        let body = r#"{"source_peer":"10.42.0.1","target_peer":"10.42.0.2","vm_id":"abc","disk_path":"/var/lib/mde-vms/abc.qcow2"}"#;
        let req = parse_migrate_request(body).expect("parse");
        assert_eq!(req.source_peer, "10.42.0.1");
        assert_eq!(req.target_peer, "10.42.0.2");
        assert_eq!(req.vm_id, "abc");
        assert_eq!(req.disk_path, "/var/lib/mde-vms/abc.qcow2");
    }

    #[test]
    fn parse_migrate_rejects_malformed_json() {
        let err = parse_migrate_request("nope").expect_err("malformed");
        assert!(err.contains("malformed"));
    }

    #[test]
    fn migration_wire_admission_rejects_duplicate_keys_traversal_and_oversize() {
        let duplicate = r#"{"source_peer":"10.42.0.1","source_peer":"10.42.0.9","target_peer":"10.42.0.2","vm_id":"abc","disk_path":"/var/lib/mde-vms/abc.qcow2"}"#;
        assert!(parse_migrate_request(duplicate).is_err());
        let traversal = r#"{"source_peer":"10.42.0.1","target_peer":"10.42.0.2","vm_id":"abc","disk_path":"/var/lib/mde-vms/../shadow"}"#;
        assert!(parse_migrate_request(traversal).is_err());
        let oversized = "x".repeat(MAX_MIGRATION_WIRE_BYTES + 1);
        assert!(parse_migrate_request(&oversized).is_err());
    }

    // ── is_source_peer ──

    #[test]
    fn is_source_peer_true_when_match() {
        let req = MigrateRequest {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "abc".into(),
            disk_path: "/d".into(),
        };
        assert!(is_source_peer(&req, "10.42.0.1"));
    }

    #[test]
    fn is_source_peer_false_when_mismatch() {
        let req = MigrateRequest {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "abc".into(),
            disk_path: "/d".into(),
        };
        assert!(!is_source_peer(&req, "10.42.0.99"));
    }

    #[test]
    fn is_source_peer_false_when_own_ip_empty() {
        let req = MigrateRequest {
            source_peer: "".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "abc".into(),
            disk_path: "/d".into(),
        };
        // Empty source_peer + empty own_ip would otherwise spuriously
        // match — explicit guard.
        assert!(!is_source_peer(&req, ""));
    }

    // ── rsync args ──

    #[test]
    fn rsync_args_use_compress_and_overlay_target() {
        let args = build_rsync_args(
            "/var/lib/mde-vms/abc.qcow2",
            "10.42.0.2",
            "/var/lib/mde-vms/",
        );
        assert!(args.contains(&"--compress".to_string()));
        assert!(args.contains(&"--progress".to_string()));
        assert!(args.contains(&"/var/lib/mde-vms/abc.qcow2".to_string()));
        assert_eq!(args.last().unwrap(), "10.42.0.2:/var/lib/mde-vms/");
    }

    // ── target_disk_path_for ──

    #[test]
    fn target_disk_path_handles_trailing_slash() {
        let p = target_disk_path_for("/var/lib/mde-vms/abc.qcow2", "/var/lib/mde-vms/");
        assert_eq!(p, "/var/lib/mde-vms/abc.qcow2");
    }

    #[test]
    fn target_disk_path_inserts_separator_when_missing() {
        let p = target_disk_path_for("/src/abc.qcow2", "/var/lib/mde-vms");
        assert_eq!(p, "/var/lib/mde-vms/abc.qcow2");
    }

    // ── migrate-ready event ──

    #[test]
    fn migrate_ready_event_carries_correlation_ulid() {
        let req = MigrateRequest {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "abc".into(),
            disk_path: "/var/lib/mde-vms/abc.qcow2".into(),
        };
        let ev = build_migrate_ready_event(
            &req,
            "/var/lib/mde-vms/abc.qcow2".into(),
            "01JAN".into(),
            "<domain>…</domain>".into(),
        );
        assert_eq!(ev.target_peer, "10.42.0.2");
        assert_eq!(ev.request_ulid, "01JAN");
        assert_eq!(ev.target_disk_path, "/var/lib/mde-vms/abc.qcow2");
        assert_eq!(ev.domain_xml, "<domain>…</domain>");
    }

    #[test]
    fn is_target_peer_filters_by_target() {
        let ev = MigrateReadyEvent {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "abc".into(),
            target_disk_path: "/var/lib/mde-vms/abc.qcow2".into(),
            request_ulid: "01JAN".into(),
            domain_xml: "<domain/>".into(),
        };
        assert!(is_target_peer(&ev, "10.42.0.2"));
        assert!(!is_target_peer(&ev, "10.42.0.1"));
        assert!(!is_target_peer(&ev, ""));
    }

    #[test]
    fn migrate_ready_event_round_trips_domain_xml() {
        let ev = MigrateReadyEvent {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "abc".into(),
            target_disk_path: "/var/lib/mde-vms/abc.qcow2".into(),
            request_ulid: "01JAN".into(),
            domain_xml: "<domain type='kvm'><name>abc</name></domain>".into(),
        };
        let body = serde_json::to_string(&ev).unwrap();
        let back = parse_migrate_ready_event(&body).expect("parse");
        assert_eq!(back, ev);
        assert!(back.domain_xml.contains("<name>abc</name>"));
    }

    #[test]
    fn parse_migrate_ready_rejects_malformed() {
        assert!(parse_migrate_ready_event("not json").is_err());
    }

    // ── Required scenario 4 (VIRT-8.b half): target-provision failure ──

    #[test]
    fn run_migrate_target_errors_on_empty_domain_xml() {
        // Empty domain_xml means the source dumpxml failed — the
        // target must surface a clear error (→ migrate-failed), not
        // silently define nothing.
        let ev = MigrateReadyEvent {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "abc".into(),
            target_disk_path: "/var/lib/mde-vms/abc.qcow2".into(),
            request_ulid: "01JAN".into(),
            domain_xml: "   ".into(),
        };
        let actuator = FakeMigrationActuator::default();
        let err = run_migrate_target(&ev, &actuator).expect_err("empty xml must fail");
        assert!(err.contains("definition is empty"), "{err}");
    }

    #[test]
    fn migrate_failed_event_shape() {
        let ev = MigrateReadyEvent {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "abc".into(),
            target_disk_path: "/d".into(),
            request_ulid: "01JAN".into(),
            domain_xml: "<domain/>".into(),
        };
        let failed = MigrateFailedEvent {
            vm_id: ev.vm_id.clone(),
            target_peer: ev.target_peer.clone(),
            request_ulid: ev.request_ulid.clone(),
            error: "virsh define failed for abc".into(),
        };
        let body = serde_json::to_string(&failed).unwrap();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["vm_id"], "abc");
        assert_eq!(v["target_peer"], "10.42.0.2");
        assert!(v["error"].as_str().unwrap().contains("virsh define"));
    }

    #[test]
    fn migrate_ready_and_failed_topics_under_event_prefix() {
        assert!(MIGRATE_READY_TOPIC.starts_with("event/"));
        assert!(MIGRATE_FAILED_TOPIC.starts_with("event/"));
    }

    // ── Required scenario 3: rsync failure (via the MigrationOutcome
    //    variant + the test that run_migration would surface it; we
    //    cover the failure-shape here without invoking rsync) ──

    #[test]
    fn migration_outcome_rsync_failure_carries_description() {
        let out = MigrationOutcome::RsyncFailure {
            exit_description: "rsync exited 23".into(),
        };
        match out {
            MigrationOutcome::RsyncFailure { exit_description } => {
                assert!(exit_description.contains("23"));
            }
            _ => panic!("wrong variant"),
        }
    }

    // ── Required scenario 1: happy path planning ──

    #[test]
    fn migration_power_and_definition_paths_use_workload_adapter() {
        let req = MigrateRequest {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "abc-uuid".into(),
            disk_path: "/var/lib/mde-vms/abc-uuid.qcow2".into(),
        };
        assert!(is_source_peer(&req, "10.42.0.1"));
        let actuator = FakeMigrationActuator {
            stop_error: true,
            ..FakeMigrationActuator::default()
        };
        assert!(matches!(
            run_migration(&req, &actuator),
            MigrationOutcome::AuthorityFailure { .. }
        ));
        assert_eq!(
            actuator.calls(),
            ["capture:abc-uuid", "stop:abc-uuid", "define-start:abc-uuid"]
        );
    }

    #[test]
    fn compute_migrate_has_no_direct_libvirt_lifecycle_subprocess() {
        let production = include_str!("compute_migrate.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production source");
        assert!(!production.contains("Command::new(\"virsh\")"));
        assert!(!production.contains("run_virsh"));
        assert!(!production.contains("SystemWorkloadActuator"));
    }

    #[test]
    fn target_rollback_and_commit_cleanup_route_through_workload_adapter() {
        let actuator = FakeMigrationActuator::default();
        let event = MigrateReadyEvent {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "vm-adapter-route".into(),
            target_disk_path: "/var/lib/mde-vms/vm-adapter-route.qcow2".into(),
            request_ulid: "01ADAPTER".into(),
            domain_xml: "<domain><name>vm-adapter-route</name></domain>".into(),
        };

        run_migrate_target(&event, &actuator).expect("target define/start");
        run_source_rollback(&event.vm_id, &event.domain_xml, &actuator).expect("source rollback");
        run_source_undefine(&event.vm_id, &actuator).expect("source relinquish");

        assert_eq!(
            actuator.calls(),
            [
                "define-start:vm-adapter-route",
                "define-start:vm-adapter-route",
                "relinquish:vm-adapter-route",
            ]
        );
    }

    // ── ACTION_TOPIC prefix lock ──

    #[test]
    fn action_topic_under_action_prefix() {
        assert!(ACTION_TOPIC.starts_with("action/"));
    }

    #[test]
    fn migrate_ready_topic_under_event_prefix() {
        assert!(MIGRATE_READY_TOPIC.starts_with("event/"));
    }

    // ── mackesd-02: rsync bound + off-runtime drain seam ──

    #[test]
    fn rsync_timeout_is_generous_but_finite() {
        // A multi-GiB disk ship legitimately needs minutes, so the bound must be
        // large — but finite so a wedged rsync can't block forever (mackesd-02).
        assert!(RSYNC_TIMEOUT >= Duration::from_secs(300));
        assert!(RSYNC_TIMEOUT.as_secs() > 0);
    }

    #[test]
    fn drain_source_jobs_returns_only_this_peers_requests_and_advances_cursor() {
        // The sync drain seam (which lets run_migration move to spawn_blocking)
        // returns only the requests this peer is the SOURCE for, and advances the
        // cursor past EVERY message — so a hung/slow migration off-runtime never
        // re-drives already-consumed messages.
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).expect("persist");
        let mine_req = MigrateRequest {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "vm-mine".into(),
            disk_path: "/var/lib/mde-vms/vm-mine.qcow2".into(),
        };
        let mine = authorized_request_body(&mine_req, "drain-mine");
        let other = r#"{"source_peer":"10.42.0.9","target_peer":"10.42.0.2","vm_id":"vm-other","disk_path":"/d"}"#;
        persist
            .write(ACTION_TOPIC, Priority::Default, None, Some(&mine))
            .unwrap();
        persist
            .write(ACTION_TOPIC, Priority::Default, None, Some(other))
            .unwrap();
        let worker = test_worker(tmp.path().join("auth").as_path());
        let mut cursor = None;
        let jobs = drain_source_jobs(&persist, &worker, &mut cursor);
        assert_eq!(jobs.len(), 1, "only the source-peer request is returned");
        assert_eq!(jobs[0].1.vm_id, "vm-mine");
        // Cursor advanced past BOTH messages → a second drain is empty.
        assert!(cursor.is_some());
        assert!(drain_source_jobs(&persist, &worker, &mut cursor).is_empty());
    }

    #[test]
    fn unsigned_source_request_is_refused_before_backend_dispatch() {
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).expect("persist");
        let req = MigrateRequest {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "vm-unsigned".into(),
            disk_path: "/var/lib/mde-vms/vm-unsigned.qcow2".into(),
        };
        let unsigned = request_body(&req);
        persist
            .write(ACTION_TOPIC, Priority::Default, None, Some(&unsigned))
            .unwrap();
        let worker = test_worker(tmp.path().join("auth").as_path());
        let mut cursor = None;
        // An empty dispatch set is the no-backend proof: the caller only passes
        // returned jobs to the virsh/rsync runner.
        assert!(drain_source_jobs(&persist, &worker, &mut cursor).is_empty());
    }

    #[test]
    fn tampered_source_request_is_refused_but_original_capability_remains_valid() {
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).expect("persist");
        let req = MigrateRequest {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "vm-tamper".into(),
            disk_path: "/var/lib/mde-vms/vm-tamper.qcow2".into(),
        };
        let armed = authorized_request_body(&req, "tamper-once");
        let tampered = armed.replace("vm-tamper", "vm-other");
        persist
            .write(ACTION_TOPIC, Priority::Default, None, Some(&tampered))
            .unwrap();
        persist
            .write(ACTION_TOPIC, Priority::Default, None, Some(&armed))
            .unwrap();
        let worker = test_worker(tmp.path().join("auth").as_path());
        let mut cursor = None;
        let jobs = drain_source_jobs(&persist, &worker, &mut cursor);
        assert_eq!(jobs.len(), 1, "tamper must not consume the valid nonce");
        assert_eq!(jobs[0].1.vm_id, "vm-tamper");
    }

    #[test]
    fn replayed_source_request_is_refused_after_first_dispatch() {
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).expect("persist");
        let req = MigrateRequest {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "vm-replay".into(),
            disk_path: "/var/lib/mde-vms/vm-replay.qcow2".into(),
        };
        let armed = authorized_request_body(&req, "replay-once");
        persist
            .write(ACTION_TOPIC, Priority::Default, None, Some(&armed))
            .unwrap();
        persist
            .write(ACTION_TOPIC, Priority::Default, None, Some(&armed))
            .unwrap();
        let worker = test_worker(tmp.path().join("auth").as_path());
        let mut cursor = None;
        let jobs = drain_source_jobs(&persist, &worker, &mut cursor);
        assert_eq!(jobs.len(), 1, "a capability nonce is single-use");
    }

    fn ready_event_for_tests() -> MigrateReadyEvent {
        MigrateReadyEvent {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "vm-ready-auth".into(),
            target_disk_path: "/var/lib/mde-vms/vm-ready-auth.qcow2".into(),
            request_ulid: "01READYAUTH".into(),
            domain_xml: "<domain><name>vm-ready-auth</name></domain>".into(),
        }
    }

    #[test]
    fn unsigned_ready_event_is_refused_before_target_backend_dispatch() {
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).expect("persist");
        let event = ready_event_for_tests();
        let unsigned = serde_json::to_string(&event).unwrap();
        persist
            .write(
                MIGRATE_READY_TOPIC,
                Priority::Default,
                None,
                Some(&unsigned),
            )
            .unwrap();
        let worker = test_worker_at(tmp.path().join("auth").as_path(), "10.42.0.2");
        let mut cursor = None;
        // No event reaches run_migrate_target, so no virsh define/start can run.
        assert!(drain_target_jobs(&persist, &worker, &mut cursor).is_empty());
    }

    #[test]
    fn tampered_ready_event_is_refused_without_consuming_valid_event_nonce() {
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).expect("persist");
        let event = ready_event_for_tests();
        let armed = authorized_ready_body(&event, "ready-tamper-once");
        let tampered = armed.replace("vm-ready-auth", "vm-ready-other");
        persist
            .write(
                MIGRATE_READY_TOPIC,
                Priority::Default,
                None,
                Some(&tampered),
            )
            .unwrap();
        persist
            .write(MIGRATE_READY_TOPIC, Priority::Default, None, Some(&armed))
            .unwrap();
        let worker = test_worker_at(tmp.path().join("auth").as_path(), "10.42.0.2");
        let mut cursor = None;
        let jobs = drain_target_jobs(&persist, &worker, &mut cursor);
        assert_eq!(jobs.len(), 1, "tamper must not consume the valid nonce");
        assert_eq!(jobs[0].vm_id, event.vm_id);
    }

    #[test]
    fn replayed_ready_event_is_refused_after_first_target_dispatch() {
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).expect("persist");
        let event = ready_event_for_tests();
        let armed = authorized_ready_body(&event, "ready-replay-once");
        for _ in 0..2 {
            persist
                .write(MIGRATE_READY_TOPIC, Priority::Default, None, Some(&armed))
                .unwrap();
        }
        let worker = test_worker_at(tmp.path().join("auth").as_path(), "10.42.0.2");
        let mut cursor = None;
        let jobs = drain_target_jobs(&persist, &worker, &mut cursor);
        assert_eq!(jobs.len(), 1, "event capability nonce is single-use");
    }

    #[test]
    fn unsigned_commit_and_failure_events_cannot_resolve_source_destructive_steps() {
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).expect("persist");
        let committed = MigrateCommittedEvent {
            vm_id: "vm-complete-auth".into(),
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            request_ulid: "01COMPLETEAUTH".into(),
        };
        let failed = MigrateFailedEvent {
            vm_id: committed.vm_id.clone(),
            target_peer: committed.target_peer.clone(),
            request_ulid: committed.request_ulid.clone(),
            error: "target refused".into(),
        };
        persist
            .write(
                MIGRATE_COMMITTED_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&committed).unwrap()),
            )
            .unwrap();
        persist
            .write(
                MIGRATE_FAILED_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&failed).unwrap()),
            )
            .unwrap();
        let worker = test_worker(tmp.path().join("auth").as_path());
        let mut committed_cursor = None;
        let mut failed_cursor = None;
        let committed_events =
            drain_committed_events(&persist, &mut committed_cursor, worker.authorizer.as_ref());
        let failed_events =
            drain_failed_events(&persist, &mut failed_cursor, worker.authorizer.as_ref());
        assert!(committed_events.is_empty());
        assert!(failed_events.is_empty());
        assert_eq!(
            classify_commit(&committed.request_ulid, &[], &[], false),
            CommitResolution::Pending,
            "without an authenticated receipt the source must not undefine or roll back"
        );
    }

    #[test]
    fn authenticated_commit_and_failure_events_are_admitted_once() {
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).expect("persist");
        let committed = MigrateCommittedEvent {
            vm_id: "vm-complete-auth-ok".into(),
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            request_ulid: "01COMPLETEAUTHOK".into(),
        };
        let failed = MigrateFailedEvent {
            vm_id: committed.vm_id.clone(),
            target_peer: committed.target_peer.clone(),
            request_ulid: "01FAILAUTHOK".into(),
            error: "target refused".into(),
        };
        let committed_body = authorized_committed_body(&committed, "commit-auth-once");
        let failed_body = authorized_failed_body(&failed, "failed-auth-once");
        persist
            .write(
                MIGRATE_COMMITTED_TOPIC,
                Priority::Default,
                None,
                Some(&committed_body),
            )
            .unwrap();
        persist
            .write(
                MIGRATE_FAILED_TOPIC,
                Priority::Default,
                None,
                Some(&failed_body),
            )
            .unwrap();
        let worker = test_worker(tmp.path().join("auth").as_path());
        let mut committed_cursor = None;
        let mut failed_cursor = None;
        let committed_events =
            drain_committed_events(&persist, &mut committed_cursor, worker.authorizer.as_ref());
        let failed_events =
            drain_failed_events(&persist, &mut failed_cursor, worker.authorizer.as_ref());
        assert_eq!(committed_events, vec![committed]);
        assert_eq!(failed_events, vec![failed]);
    }

    #[test]
    fn published_event_envelopes_pass_the_receiving_gates() {
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).expect("persist");
        let signer = CloudArmSigner::new(AUTH_KEY.to_vec()).expect("test signer");
        let ready = ready_event_for_tests();
        let committed = build_migrate_committed_event(&ready);
        let failed = MigrateFailedEvent {
            vm_id: ready.vm_id.clone(),
            target_peer: ready.target_peer.clone(),
            request_ulid: ready.request_ulid.clone(),
            error: "target refused".into(),
        };

        publish_authorized_migrate_event(
            &persist,
            MIGRATE_READY_TOPIC,
            &ready,
            COMPUTE_MIGRATE_READY_AUTH_VERB,
            &migration_event_auth_target(&ready.vm_id, &ready.source_peer, &ready.target_peer),
            &signer,
            "emitted-ready-0123456789abcdef0123456789",
            AUTH_NOW_MS,
        )
        .expect("publish ready");
        publish_authorized_migrate_event(
            &persist,
            MIGRATE_COMMITTED_TOPIC,
            &committed,
            COMPUTE_MIGRATE_COMMITTED_AUTH_VERB,
            &migration_event_auth_target(
                &committed.vm_id,
                &committed.source_peer,
                &committed.target_peer,
            ),
            &signer,
            "emitted-committed-0123456789abcdef0123",
            AUTH_NOW_MS,
        )
        .expect("publish committed");
        publish_authorized_migrate_event(
            &persist,
            MIGRATE_FAILED_TOPIC,
            &failed,
            COMPUTE_MIGRATE_FAILED_AUTH_VERB,
            &migration_event_auth_target(&failed.vm_id, "", &failed.target_peer),
            &signer,
            "emitted-failed-0123456789abcdef012345",
            AUTH_NOW_MS,
        )
        .expect("publish failed");

        for topic in [
            MIGRATE_READY_TOPIC,
            MIGRATE_COMMITTED_TOPIC,
            MIGRATE_FAILED_TOPIC,
        ] {
            let messages = persist.list_since(topic, None).expect("list emitted event");
            let body = messages
                .last()
                .and_then(|message| message.body.as_deref())
                .expect("emitted event body");
            let document: serde_json::Value = serde_json::from_str(body).expect("event JSON");
            assert_eq!(document["schema_version"], serde_json::json!(1));
            assert!(document["armed_token"].as_str().is_some());
        }

        let worker = test_worker_at(tmp.path().join("auth").as_path(), "10.42.0.2");
        let mut ready_cursor = None;
        let mut committed_cursor = None;
        let mut failed_cursor = None;
        assert_eq!(
            drain_target_jobs(&persist, &worker, &mut ready_cursor),
            vec![ready]
        );
        assert_eq!(
            drain_committed_events(&persist, &mut committed_cursor, worker.authorizer.as_ref()),
            vec![committed]
        );
        assert_eq!(
            drain_failed_events(&persist, &mut failed_cursor, worker.authorizer.as_ref()),
            vec![failed]
        );
    }

    #[test]
    fn unsigned_tampered_and_replayed_commit_and_failure_events_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).expect("persist");
        let committed = MigrateCommittedEvent {
            vm_id: "vm-negative-auth".into(),
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            request_ulid: "01NEGATIVEAUTH".into(),
        };
        let failed = MigrateFailedEvent {
            vm_id: committed.vm_id.clone(),
            target_peer: committed.target_peer.clone(),
            request_ulid: committed.request_ulid.clone(),
            error: "target refused".into(),
        };
        let committed_body =
            authorized_committed_body(&committed, "negative-committed-0123456789abcdef0123");
        let failed_body = authorized_failed_body(&failed, "negative-failed-0123456789abcdef012345");
        let tampered_committed = committed_body.replace("vm-negative-auth", "vm-tampered-auth");
        let tampered_failed = failed_body.replace("target refused", "tampered failure");

        persist
            .write(
                MIGRATE_COMMITTED_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&committed).unwrap()),
            )
            .unwrap();
        for body in [&tampered_committed, &committed_body, &committed_body] {
            persist
                .write(MIGRATE_COMMITTED_TOPIC, Priority::Default, None, Some(body))
                .unwrap();
        }
        persist
            .write(
                MIGRATE_FAILED_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&failed).unwrap()),
            )
            .unwrap();
        for body in [&tampered_failed, &failed_body, &failed_body] {
            persist
                .write(MIGRATE_FAILED_TOPIC, Priority::Default, None, Some(body))
                .unwrap();
        }

        let worker = test_worker(tmp.path().join("auth").as_path());
        let mut committed_cursor = None;
        let mut failed_cursor = None;
        assert_eq!(
            drain_committed_events(&persist, &mut committed_cursor, worker.authorizer.as_ref()),
            vec![committed]
        );
        assert_eq!(
            drain_failed_events(&persist, &mut failed_cursor, worker.authorizer.as_ref()),
            vec![failed]
        );
        assert!(drain_committed_events(
            &persist,
            &mut committed_cursor,
            worker.authorizer.as_ref()
        )
        .is_empty());
        assert!(
            drain_failed_events(&persist, &mut failed_cursor, worker.authorizer.as_ref())
                .is_empty()
        );
    }

    // ── vdi-vm-5: deferred undefine behind a target commit ack ──

    #[test]
    fn migrate_committed_topic_under_event_prefix() {
        assert!(MIGRATE_COMMITTED_TOPIC.starts_with("event/"));
    }

    #[test]
    fn commit_timeout_is_generous_but_finite() {
        // The target must drain migrate-ready, define + boot the guest, and ack
        // across the overlay — so the bound is generous, but finite so a target
        // that never comes up can't strand the source forever (vdi-vm-5).
        assert!(DEFAULT_COMMIT_TIMEOUT >= Duration::from_secs(60));
        assert!(DEFAULT_COMMIT_TIMEOUT.as_secs() > 0);
    }

    #[test]
    fn build_migrate_committed_preserves_correlation() {
        let ready = MigrateReadyEvent {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "abc".into(),
            target_disk_path: "/var/lib/mde-vms/abc.qcow2".into(),
            request_ulid: "01JANULID".into(),
            domain_xml: "<domain/>".into(),
        };
        let committed = build_migrate_committed_event(&ready);
        assert_eq!(committed.vm_id, "abc");
        assert_eq!(committed.source_peer, "10.42.0.1");
        assert_eq!(committed.target_peer, "10.42.0.2");
        // The correlation ULID must survive so the source matches its pending
        // commit and undefines the right domain.
        assert_eq!(committed.request_ulid, "01JANULID");
    }

    #[test]
    fn migrate_committed_event_round_trips() {
        let ev = MigrateCommittedEvent {
            vm_id: "abc".into(),
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            request_ulid: "01JAN".into(),
        };
        let body = serde_json::to_string(&ev).unwrap();
        let back = parse_migrate_committed_event(&body).expect("parse");
        assert_eq!(back, ev);
    }

    #[test]
    fn parse_migrate_committed_rejects_malformed() {
        assert!(parse_migrate_committed_event("not json").is_err());
    }

    #[test]
    fn parse_migrate_failed_round_trips() {
        // The source now CONSUMES migrate-failed to roll back, so it needs a
        // parser that round-trips the target's published shape (vdi-vm-5).
        let ev = MigrateFailedEvent {
            vm_id: "abc".into(),
            target_peer: "10.42.0.2".into(),
            request_ulid: "01JAN".into(),
            error: "virsh define failed for abc".into(),
        };
        let body = serde_json::to_string(&ev).unwrap();
        let back = parse_migrate_failed_event(&body).expect("parse");
        assert_eq!(back, ev);
        assert!(back.error.contains("virsh define"));
    }

    // ── classify_commit: the source-side decision core ──

    #[test]
    fn classify_commit_pending_when_no_signal() {
        // Required scenario 1 (ordering): before the target acks, the source
        // must NOT undefine — the domain stays the shut-off rollback anchor.
        let r = classify_commit("01JAN", &[], &[], false);
        assert_eq!(r, CommitResolution::Pending);
    }

    #[test]
    fn classify_commit_undefines_only_after_committed() {
        // Required scenario 1: the destructive undefine is authorized ONLY once
        // migrate-committed carrying the matching ULID is observed.
        let before = classify_commit("01JAN", &[], &[], false);
        assert_eq!(before, CommitResolution::Pending, "no undefine pre-commit");
        let after = classify_commit("01JAN", &["01JAN".into()], &[], false);
        assert_eq!(after, CommitResolution::Undefine);
        // A commit for a DIFFERENT migration must not undefine this one.
        let other = classify_commit("01JAN", &["09ZZZ".into()], &[], false);
        assert_eq!(other, CommitResolution::Pending);
    }

    #[test]
    fn classify_commit_rolls_back_on_target_failure() {
        // Required scenario: target-failure → source rolls back (VM not lost).
        let r = classify_commit(
            "01JAN",
            &[],
            &[("01JAN".into(), "virsh start failed for abc".into())],
            false,
        );
        match r {
            CommitResolution::RollBack {
                reason: RollbackReason::TargetFailed { error },
            } => assert!(error.contains("virsh start failed")),
            other => panic!("expected TargetFailed rollback, got {other:?}"),
        }
    }

    #[test]
    fn classify_commit_rolls_back_on_commit_timeout() {
        // Required scenario: commit-timeout → same rollback path.
        let r = classify_commit("01JAN", &[], &[], true);
        assert_eq!(
            r,
            CommitResolution::RollBack {
                reason: RollbackReason::CommitTimeout
            }
        );
    }

    #[test]
    fn classify_commit_committed_beats_failed() {
        // If both a commit and a (stale) failure are seen, the VM is confirmed
        // up on the target, so undefine is safe — commit wins over rollback.
        let r = classify_commit(
            "01JAN",
            &["01JAN".into()],
            &[("01JAN".into(), "spurious".into())],
            true,
        );
        assert_eq!(r, CommitResolution::Undefine);
    }

    #[test]
    fn run_source_rollback_errors_on_empty_xml_deterministically() {
        // Rollback re-defines from the retained dumpxml; an empty XML (source
        // dumpxml had failed) is rejected before touching the environment, so
        // this is deterministic whether or not virsh is installed.
        let actuator = FakeMigrationActuator::default();
        let err = run_source_rollback("abc", "   ", &actuator).expect_err("empty xml must fail");
        assert!(err.contains("definition is empty"), "{err}");
    }

    #[test]
    fn retained_dumpxml_round_trips_for_rollback() {
        // Required scenario: the retained dumpxml round-trips. The source
        // captures dumpxml before shutdown, ships it in migrate-ready, and
        // retains the SAME bytes to re-define on rollback — prove the XML
        // survives the migrate-ready wire hop verbatim so rollback recreates
        // the identical domain.
        let xml = "<domain type='kvm'><name>abc</name><vcpu>4</vcpu></domain>";
        let req = MigrateRequest {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "abc".into(),
            disk_path: "/var/lib/mde-vms/abc.qcow2".into(),
        };
        let ready = build_migrate_ready_event(
            &req,
            target_disk_path_for(&req.disk_path, DEFAULT_TARGET_VM_DIR),
            "01JAN".into(),
            xml.to_string(),
        );
        let body = serde_json::to_string(&ready).unwrap();
        let back = parse_migrate_ready_event(&body).expect("parse");
        // The bytes the target would define AND the bytes the source retains for
        // rollback are byte-identical to the captured dumpxml.
        assert_eq!(back.domain_xml, xml);
        assert!(back.domain_xml.contains("<vcpu>4</vcpu>"));
    }

    #[test]
    fn drain_committed_events_advances_cursor_and_returns_all() {
        // Same at-least-once drain shape as the request/ready drains: every
        // committed message advances the cursor, and a second drain is empty.
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).expect("persist");
        let a = MigrateCommittedEvent {
            vm_id: "vm-a".into(),
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            request_ulid: "01A".into(),
        };
        let b = MigrateCommittedEvent {
            vm_id: "vm-b".into(),
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            request_ulid: "01B".into(),
        };
        let auth_root = tmp.path().join("auth");
        let authorizer = ActionAuthorizer::for_test(AUTH_KEY, auth_root, AUTH_NOW_MS);
        for ev in [&a, &b] {
            let body = authorized_committed_body(ev, &format!("drain-{}", ev.request_ulid));
            persist
                .write(
                    MIGRATE_COMMITTED_TOPIC,
                    Priority::Default,
                    None,
                    Some(&body),
                )
                .unwrap();
        }
        let mut cursor = None;
        let drained = drain_committed_events(&persist, &mut cursor, &authorizer);
        assert_eq!(drained.len(), 2);
        let ulids: Vec<&str> = drained.iter().map(|e| e.request_ulid.as_str()).collect();
        assert!(ulids.contains(&"01A") && ulids.contains(&"01B"));
        assert!(cursor.is_some());
        assert!(drain_committed_events(&persist, &mut cursor, &authorizer).is_empty());
    }

    #[test]
    fn full_commit_lifecycle_undefines_then_next_drain_empty() {
        // End-to-end (headless) source-side lifecycle: a pending commit stays
        // Pending until its committed event lands, then resolves to Undefine.
        // Proves the deferred-undefine ordering with a real Bus (vdi-vm-5).
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).expect("persist");
        let mut committed_cursor = None;
        let authorizer = ActionAuthorizer::for_test(AUTH_KEY, tmp.path().join("auth"), AUTH_NOW_MS);

        // Tick 1: no ack yet → Pending (source keeps the shut-off anchor).
        let acks = drain_committed_events(&persist, &mut committed_cursor, &authorizer);
        let ulids: Vec<String> = acks.into_iter().map(|e| e.request_ulid).collect();
        assert_eq!(
            classify_commit("01JAN", &ulids, &[], false),
            CommitResolution::Pending
        );

        // Target commits.
        let committed = MigrateCommittedEvent {
            vm_id: "abc".into(),
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            request_ulid: "01JAN".into(),
        };
        let committed_body = authorized_committed_body(&committed, "lifecycle-commit");
        persist
            .write(
                MIGRATE_COMMITTED_TOPIC,
                Priority::Default,
                None,
                Some(&committed_body),
            )
            .unwrap();

        // Tick 2: ack observed → Undefine authorized.
        let acks = drain_committed_events(&persist, &mut committed_cursor, &authorizer);
        let ulids: Vec<String> = acks.into_iter().map(|e| e.request_ulid).collect();
        assert_eq!(
            classify_commit("01JAN", &ulids, &[], false),
            CommitResolution::Undefine
        );
    }

    fn durable_pending(phase: PendingPhase) -> PendingCommit {
        let request = MigrateRequest {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "vm-ledger".into(),
            disk_path: "/var/lib/mde-vms/vm-ledger.qcow2".into(),
        };
        let ready_event = build_migrate_ready_event(
            &request,
            request.disk_path.clone(),
            "01LEDGER".into(),
            "<domain><name>vm-ledger</name></domain>".into(),
        );
        PendingCommit {
            request_ulid: "01LEDGER".into(),
            vm_id: request.vm_id,
            domain_xml: ready_event.domain_xml.clone(),
            ready_event,
            ready_body: None,
            deadline_ms: 20_000,
            phase,
            next_attempt_ms: 10_000,
        }
    }

    #[test]
    fn migration_ledger_recovers_cursors_jobs_and_terminal_retry_phase() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ledger = MigrationLedger::open(tmp.path()).expect("open ledger");
        ledger.state.source_cursor = Some("01SOURCE".into());
        ledger.state.target_cursor = Some("01TARGET".into());
        ledger.state.committed_cursor = Some("01COMMIT".into());
        ledger.state.failed_cursor = Some("01FAILED".into());
        ledger
            .state
            .pending_commits
            .push(durable_pending(PendingPhase::Rollback {
                reason: "target failed".into(),
            }));
        ledger.store().expect("store ledger");

        let recovered = MigrationLedger::open(tmp.path()).expect("recover ledger");
        assert_eq!(recovered.state.source_cursor.as_deref(), Some("01SOURCE"));
        assert_eq!(recovered.state.target_cursor.as_deref(), Some("01TARGET"));
        assert_eq!(
            recovered.state.committed_cursor.as_deref(),
            Some("01COMMIT")
        );
        assert_eq!(recovered.state.failed_cursor.as_deref(), Some("01FAILED"));
        assert!(matches!(
            recovered.state.pending_commits[0].phase,
            PendingPhase::Rollback { .. }
        ));
        assert_eq!(recovered.state.pending_commits[0].next_attempt_ms, 10_000);
    }

    #[test]
    fn source_admission_persists_authorized_job_and_cursor_before_effect() {
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().join("bus")).expect("persist");
        let request = MigrateRequest {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "vm-admitted".into(),
            disk_path: "/var/lib/mde-vms/vm-admitted.qcow2".into(),
        };
        let body = authorized_request_body(&request, "durable-source-admission");
        persist
            .write(ACTION_TOPIC, Priority::Default, None, Some(&body))
            .expect("publish request");
        let worker = test_worker(&tmp.path().join("auth"));
        let state_root = tmp.path().join("state");
        let mut ledger = MigrationLedger::open(&state_root).expect("open ledger");

        admit_source_jobs(&persist, &worker, &mut ledger).expect("admit source job");
        drop(ledger);

        let recovered = MigrationLedger::open(&state_root).expect("recover ledger");
        assert!(recovered.state.source_cursor.is_some());
        assert_eq!(recovered.state.source_jobs.len(), 1);
        assert!(recovered.state.source_jobs[0].authorized);
        assert_eq!(recovered.state.source_jobs[0].request.vm_id, "vm-admitted");
    }

    #[test]
    fn committed_ack_atomically_recovers_as_relinquish_retry() {
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().join("bus")).expect("persist");
        let state_root = tmp.path().join("state");
        let mut ledger = MigrationLedger::open(&state_root).expect("open ledger");
        ledger
            .state
            .pending_commits
            .push(durable_pending(PendingPhase::Waiting));
        ledger.store().expect("store pending commit");
        let committed = MigrateCommittedEvent {
            vm_id: "vm-ledger".into(),
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            request_ulid: "01LEDGER".into(),
        };
        let body = authorized_committed_body(&committed, "durable-commit-ack");
        persist
            .write(
                MIGRATE_COMMITTED_TOPIC,
                Priority::Default,
                None,
                Some(&body),
            )
            .expect("publish committed ack");
        let worker = test_worker(&tmp.path().join("auth"));

        admit_ack_jobs(&persist, &worker, &mut ledger).expect("admit committed ack");
        apply_ack_jobs(&mut ledger, AUTH_NOW_MS).expect("checkpoint relinquish phase");
        drop(ledger);

        let recovered = MigrationLedger::open(&state_root).expect("recover ledger");
        assert!(recovered.state.committed_cursor.is_some());
        assert!(recovered.state.ack_jobs.is_empty());
        assert!(matches!(
            recovered.state.pending_commits[0].phase,
            PendingPhase::Relinquish
        ));
        assert_eq!(
            recovered.state.pending_commits[0].next_attempt_ms,
            AUTH_NOW_MS
        );
    }

    #[test]
    fn compute_migrate_bus_root_preserves_override_and_has_system_fallback() {
        let explicit = PathBuf::from("/tmp/compute-migrate-bus-test");
        assert_eq!(compute_migrate_bus_root(Some(explicit.clone())), explicit);
        assert_eq!(
            compute_migrate_bus_root_or_system(None),
            PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)
        );
    }

    #[tokio::test]
    async fn unavailable_bus_retries_until_shutdown_without_touching_migration_state() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let tmp = tempfile::tempdir().expect("tempdir");
        let state_root = tmp.path().join("state");
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_open = Arc::clone(&attempts);
        let authority = Arc::new(FakeMigrationActuator::default());
        let authority_for_worker: Arc<dyn MigrationAuthority> = authority.clone();
        let mut worker = test_worker(&tmp.path().join("auth"))
            .with_state_root(state_root.clone())
            .with_poll_interval(Duration::from_secs(30))
            .with_migration_authority(authority_for_worker)
            .with_bus_root_resolver(Arc::new(move || {
                attempts_for_open.fetch_add(1, Ordering::SeqCst);
                Err("injected unavailable Bus".into())
            }));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );

        for _ in 0..40 {
            if attempts.load(Ordering::SeqCst) > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert!(!task.is_finished(), "Bus absence must not end the worker");
        assert!(authority.calls().is_empty(), "no migration effect is safe");
        assert!(
            !state_root.exists(),
            "Bus activation must precede migration-ledger recovery"
        );

        shutdown_tx.send(true).expect("request shutdown");
        tokio::time::timeout(Duration::from_millis(250), task)
            .await
            .expect("shutdown must interrupt the bounded retry wait")
            .expect("worker task")
            .expect("clean worker shutdown");
    }

    #[test]
    fn bus_read_error_is_failure_and_cannot_trigger_pending_rollback() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let bus_root = tmp.path().join("bus");
        let persist = Persist::open(bus_root.clone()).expect("persist");
        let mut ledger = MigrationLedger::open(&tmp.path().join("state")).expect("ledger");
        let mut pending = durable_pending(PendingPhase::Waiting);
        pending.deadline_ms = 0;
        ledger.state.pending_commits.push(pending);
        ledger.store().expect("store expired pending migration");

        let db = rusqlite::Connection::open(bus_root.join("index.sqlite")).expect("open index");
        db.execute_batch("DROP TABLE messages;")
            .expect("inject unreadable Bus index");
        let worker = test_worker(&tmp.path().join("auth"));

        let error = admit_bus_jobs(&persist, &worker, &mut ledger)
            .expect_err("a Bus read fault must not look like an empty sweep");
        assert!(error.contains("list source migration actions"));
        assert!(ledger.state.source_cursor.is_none());
        assert!(ledger.state.source_jobs.is_empty());
        assert_eq!(ledger.state.pending_commits.len(), 1);
        assert!(matches!(
            ledger.state.pending_commits[0].phase,
            PendingPhase::Waiting
        ));
    }

    #[tokio::test]
    async fn complete_read_failure_is_effect_free_then_corrects_forward() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let bus_root = tmp.path().join("bus");
        let persist = Persist::open(bus_root.clone()).expect("persist");
        let authority = Arc::new(FakeMigrationActuator {
            calls: Mutex::new(Vec::new()),
            stop_error: true,
            ..FakeMigrationActuator::default()
        });
        let authority_for_worker: Arc<dyn MigrationAuthority> = authority.clone();
        let worker = test_worker(&tmp.path().join("auth"))
            .with_bus_root(bus_root.clone())
            .with_migration_authority(authority_for_worker);
        let mut ledger = MigrationLedger::open(&tmp.path().join("state")).expect("ledger");
        worker.cycle(&mut ledger).await.expect("activate Bus");
        let initial_cursor = ledger.state.source_cursor.clone();
        let request = MigrateRequest {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "vm-read-fault".into(),
            disk_path: "/var/lib/mde-vms/vm-read-fault.qcow2".into(),
        };
        persist
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&authorized_request_body(&request, "read-fault-forward")),
            )
            .expect("publish forward migration");
        worker
            .bus_read_failure_at
            .store(4, std::sync::atomic::Ordering::Relaxed);

        let error = worker
            .cycle(&mut ledger)
            .await
            .expect_err("final reply lane must fail the complete sweep");
        assert!(error.contains("failed read failure"));
        assert_eq!(ledger.state.source_cursor, initial_cursor);
        assert!(ledger.state.source_jobs.is_empty());
        assert!(authority.calls().is_empty());

        worker
            .cycle(&mut ledger)
            .await
            .expect("correct forward after complete read");
        assert_eq!(
            authority.calls(),
            vec![
                "capture:vm-read-fault",
                "stop:vm-read-fault",
                "define-start:vm-read-fault",
            ]
        );
    }

    #[tokio::test]
    async fn same_path_replacement_tail_skips_retained_and_runs_forward_once() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let bus_root = tmp.path().join("bus");
        let first = Persist::open(bus_root.clone()).expect("first Bus");
        let authority = Arc::new(FakeMigrationActuator {
            calls: Mutex::new(Vec::new()),
            stop_error: true,
            ..FakeMigrationActuator::default()
        });
        let authority_for_worker: Arc<dyn MigrationAuthority> = authority.clone();
        let worker = test_worker(&tmp.path().join("auth"))
            .with_bus_root(bus_root.clone())
            .with_migration_authority(authority_for_worker);
        let mut ledger = MigrationLedger::open(&tmp.path().join("state")).expect("ledger");
        worker.cycle(&mut ledger).await.expect("activate first Bus");
        drop(first);

        let replacement_root = tmp.path().join("replacement");
        let replacement = Persist::open(replacement_root.clone()).expect("replacement Bus");
        let request = MigrateRequest {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "vm-replacement".into(),
            disk_path: "/var/lib/mde-vms/vm-replacement.qcow2".into(),
        };
        replacement
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&authorized_request_body(&request, "replacement-retained")),
            )
            .expect("publish retained replacement command");
        drop(replacement);
        fs::rename(
            replacement_root.join("index.sqlite"),
            bus_root.join("index.sqlite"),
        )
        .expect("replace Bus index at same path");

        worker
            .cycle(&mut ledger)
            .await
            .expect("activate replacement Bus");
        assert!(
            authority.calls().is_empty(),
            "retained effect must not replay"
        );
        let replacement = Persist::open(bus_root.clone()).expect("open active replacement");
        replacement
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&authorized_request_body(&request, "replacement-forward")),
            )
            .expect("publish replacement forward command");
        worker
            .cycle(&mut ledger)
            .await
            .expect("execute replacement forward command");
        assert_eq!(
            authority.calls(),
            vec![
                "capture:vm-replacement",
                "stop:vm-replacement",
                "define-start:vm-replacement",
            ]
        );
    }

    #[tokio::test]
    async fn durable_exact_outbox_recovers_after_write_failure_without_repeating_effect() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let bus_root = tmp.path().join("bus");
        let persist = Persist::open(bus_root.clone()).expect("persist");
        let state_root = tmp.path().join("state");
        let auth_root = tmp.path().join("auth");
        let authority = Arc::new(FakeMigrationActuator {
            calls: Mutex::new(Vec::new()),
            stop_error: true,
            ..FakeMigrationActuator::default()
        });
        let authority_for_worker: Arc<dyn MigrationAuthority> = authority.clone();
        let worker = test_worker(&auth_root)
            .with_bus_root(bus_root.clone())
            .with_migration_authority(authority_for_worker);
        let mut ledger = MigrationLedger::open(&state_root).expect("ledger");
        worker.cycle(&mut ledger).await.expect("activate Bus");
        let request = MigrateRequest {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "vm-outbox".into(),
            disk_path: "/var/lib/mde-vms/vm-outbox.qcow2".into(),
        };
        persist
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&authorized_request_body(&request, "outbox-forward")),
            )
            .expect("publish migration");
        worker
            .bus_write_failures
            .store(1, std::sync::atomic::Ordering::Relaxed);
        worker
            .cycle(&mut ledger)
            .await
            .expect_err("first durable reply publication must fail");
        assert_eq!(authority.calls().len(), 3);
        let exact_body = ledger.state.source_jobs[0]
            .failure
            .as_ref()
            .and_then(|failure| failure.body.clone())
            .expect("exact reply body persisted before publication");
        drop(ledger);

        let authority_for_recovery: Arc<dyn MigrationAuthority> = authority.clone();
        let recovered_worker = test_worker(&auth_root)
            .with_bus_root(bus_root.clone())
            .with_migration_authority(authority_for_recovery);
        let mut recovered = MigrationLedger::open(&state_root).expect("recover ledger");
        recover_prepared_jobs(&mut recovered, recovered_worker.authorizer.as_ref())
            .expect("recover claimed transaction");
        recovered_worker
            .cycle(&mut recovered)
            .await
            .expect("publish retained exact reply");
        assert_eq!(authority.calls().len(), 3, "effect must not repeat");
        assert!(recovered.state.source_jobs.is_empty());
        let rows = persist
            .list_since(MIGRATE_FAILED_TOPIC, None)
            .expect("read recovered reply");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].body.as_deref(), Some(exact_body.as_str()));
    }

    #[tokio::test]
    async fn recovered_effect_claims_publish_indeterminate_without_repeating_backend_calls() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let bus_root = tmp.path().join("bus");
        let persist = Persist::open(bus_root.clone()).expect("persist");
        let state_root = tmp.path().join("state");
        let auth_root = tmp.path().join("auth");
        let authority = Arc::new(FakeMigrationActuator::default());
        let authority_for_worker: Arc<dyn MigrationAuthority> = authority.clone();
        let worker = test_worker(&auth_root)
            .with_bus_root(bus_root.clone())
            .with_migration_authority(authority_for_worker);
        let mut ledger = MigrationLedger::open(&state_root).expect("ledger");
        worker.cycle(&mut ledger).await.expect("activate Bus");
        let event = ready_event_for_tests();
        ledger.state.target_jobs.push(DurableTargetJob {
            message_ulid: "01TARGETCLAIM".into(),
            raw_body: authorized_ready_body(&event, "target-claimed-before-crash"),
            event,
            phase: TargetJobPhase::Applying,
            reply_body: None,
        });
        ledger
            .state
            .pending_commits
            .push(durable_pending(PendingPhase::Relinquishing));
        ledger.store().expect("persist pre-crash claims");
        drop(ledger);

        let authority_for_recovery: Arc<dyn MigrationAuthority> = authority.clone();
        let recovered_worker = test_worker(&auth_root)
            .with_bus_root(bus_root)
            .with_migration_authority(authority_for_recovery);
        let mut recovered = MigrationLedger::open(&state_root).expect("recover ledger");
        recover_prepared_jobs(&mut recovered, recovered_worker.authorizer.as_ref())
            .expect("classify interrupted claims");
        recovered_worker
            .cycle(&mut recovered)
            .await
            .expect("publish honest target failure");

        assert!(authority.calls().is_empty(), "claimed effects never repeat");
        assert!(recovered.state.target_jobs.is_empty());
        assert!(matches!(
            recovered.state.pending_commits[0].phase,
            PendingPhase::Indeterminate { .. }
        ));
        let rows = persist
            .list_since(MIGRATE_FAILED_TOPIC, None)
            .expect("read indeterminate result");
        assert_eq!(rows.len(), 1);
        let event = parse_migrate_failed_event(rows[0].body.as_deref().expect("typed body"))
            .expect("decode typed failure");
        assert!(event.error.contains("indeterminate after restart"));
    }

    #[tokio::test]
    async fn relinquish_returned_error_after_claim_is_indeterminate_and_never_retried() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let bus_root = tmp.path().join("bus");
        Persist::open(bus_root.clone()).expect("persist");
        let state_root = tmp.path().join("state");
        let auth_root = tmp.path().join("auth");
        let authority = Arc::new(FakeMigrationActuator {
            relinquish_error: true,
            ..FakeMigrationActuator::default()
        });
        let authority_for_worker: Arc<dyn MigrationAuthority> = authority.clone();
        let worker = test_worker(&auth_root)
            .with_bus_root(bus_root.clone())
            .with_migration_authority(authority_for_worker);
        let mut ledger = MigrationLedger::open(&state_root).expect("ledger");
        worker.cycle(&mut ledger).await.expect("activate Bus");
        ledger
            .state
            .pending_commits
            .push(durable_pending(PendingPhase::Relinquish));
        ledger.store().expect("persist relinquish work");

        worker
            .cycle(&mut ledger)
            .await
            .expect("record ambiguous relinquish outcome");
        assert_eq!(authority.calls(), ["relinquish:vm-ledger"]);
        assert!(matches!(
            &ledger.state.pending_commits[0].phase,
            PendingPhase::Indeterminate { operation, reason }
                if operation == "relinquish"
                    && reason.contains("returned an error after its durable effect claim")
        ));
        drop(ledger);

        let authority_for_recovery: Arc<dyn MigrationAuthority> = authority.clone();
        let recovered_worker = test_worker(&auth_root)
            .with_bus_root(bus_root)
            .with_migration_authority(authority_for_recovery);
        let mut recovered = MigrationLedger::open(&state_root).expect("recover ledger");
        recovered_worker
            .cycle(&mut recovered)
            .await
            .expect("indeterminate relinquish is not retried");
        assert_eq!(authority.calls(), ["relinquish:vm-ledger"]);
    }

    #[tokio::test]
    async fn rollback_returned_error_after_claim_is_indeterminate_and_never_retried() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let bus_root = tmp.path().join("bus");
        Persist::open(bus_root.clone()).expect("persist");
        let state_root = tmp.path().join("state");
        let auth_root = tmp.path().join("auth");
        let authority = Arc::new(FakeMigrationActuator {
            rollback_error: true,
            ..FakeMigrationActuator::default()
        });
        let authority_for_worker: Arc<dyn MigrationAuthority> = authority.clone();
        let worker = test_worker(&auth_root)
            .with_bus_root(bus_root.clone())
            .with_migration_authority(authority_for_worker);
        let mut ledger = MigrationLedger::open(&state_root).expect("ledger");
        worker.cycle(&mut ledger).await.expect("activate Bus");
        ledger
            .state
            .pending_commits
            .push(durable_pending(PendingPhase::Rollback {
                reason: "hostile target failure".into(),
            }));
        ledger.store().expect("persist rollback work");

        worker
            .cycle(&mut ledger)
            .await
            .expect("record ambiguous rollback outcome");
        assert_eq!(authority.calls(), ["define-start:vm-ledger"]);
        assert!(matches!(
            &ledger.state.pending_commits[0].phase,
            PendingPhase::Indeterminate { operation, reason }
                if operation == "rollback"
                    && reason.contains("returned an error after its durable effect claim")
        ));
        drop(ledger);

        let authority_for_recovery: Arc<dyn MigrationAuthority> = authority.clone();
        let recovered_worker = test_worker(&auth_root)
            .with_bus_root(bus_root)
            .with_migration_authority(authority_for_recovery);
        let mut recovered = MigrationLedger::open(&state_root).expect("recover ledger");
        recovered_worker
            .cycle(&mut recovered)
            .await
            .expect("indeterminate rollback is not retried");
        assert_eq!(authority.calls(), ["define-start:vm-ledger"]);
    }

    #[test]
    fn open_rejects_connection_path_identity_race_without_activation() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let tmp = tempfile::tempdir().expect("tempdir");
        let bus_root = tmp.path().join("bus");
        let original = Persist::open(bus_root.clone()).expect("original Bus");
        drop(original);
        let replacement_root = tmp.path().join("replacement-race");
        let replacement = Persist::open(replacement_root.clone()).expect("replacement Bus");
        drop(replacement);
        let swapped = Arc::new(AtomicBool::new(false));
        let swapped_for_hook = Arc::clone(&swapped);
        let replacement_index = replacement_root.join("index.sqlite");
        let worker = test_worker(&tmp.path().join("auth"))
            .with_bus_root(bus_root.clone())
            .with_bus_open_hook(Arc::new(move |root| {
                if !swapped_for_hook.swap(true, Ordering::SeqCst) {
                    fs::rename(&replacement_index, root.join("index.sqlite"))
                        .expect("inject same-path replacement race");
                }
            }));

        let error = worker
            .open_bus()
            .expect_err("stale connection/path pair must be rejected");
        assert!(error.contains("changed while opening"));
        assert!(swapped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn late_bus_tail_activates_then_executes_first_forward_migration_once() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        let tmp = tempfile::tempdir().expect("tempdir");
        let bus_root = tmp.path().join("bus");
        let persist = Persist::open(bus_root.clone()).expect("persist");
        let request = MigrateRequest {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "vm-queued-during-outage".into(),
            disk_path: "/var/lib/mde-vms/vm-queued-during-outage.qcow2".into(),
        };
        let body = authorized_request_body(&request, "queued-during-bus-outage");
        persist
            .write(ACTION_TOPIC, Priority::Default, None, Some(&body))
            .expect("queue migration while worker Bus is unavailable");
        drop(persist);

        let state_root = tmp.path().join("state");
        let mut ledger = MigrationLedger::open(&state_root).expect("ledger");
        let mut pending = durable_pending(PendingPhase::Waiting);
        pending.deadline_ms = i64::MAX;
        ledger.state.pending_commits.push(pending);
        ledger.store().expect("store pre-existing pending commit");
        drop(ledger);

        let available = Arc::new(AtomicBool::new(false));
        let attempts = Arc::new(AtomicUsize::new(0));
        let available_for_open = Arc::clone(&available);
        let attempts_for_open = Arc::clone(&attempts);
        let bus_for_open = bus_root.clone();
        let authority = Arc::new(FakeMigrationActuator {
            calls: Mutex::new(Vec::new()),
            stop_error: true,
            ..FakeMigrationActuator::default()
        });
        let authority_for_worker: Arc<dyn MigrationAuthority> = authority.clone();
        let mut worker = test_worker(&tmp.path().join("auth"))
            .with_state_root(state_root.clone())
            .with_poll_interval(Duration::from_millis(10))
            .with_migration_authority(authority_for_worker)
            .with_bus_root_resolver(Arc::new(move || {
                attempts_for_open.fetch_add(1, Ordering::SeqCst);
                if !available_for_open.load(Ordering::SeqCst) {
                    return Err("injected late Bus".into());
                }
                Ok(bus_for_open.clone())
            }));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );

        for _ in 0..40 {
            if attempts.load(Ordering::SeqCst) > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            authority.calls().is_empty(),
            "outage must have zero effects"
        );
        available.store(true, Ordering::SeqCst);

        for _ in 0..200 {
            if MigrationLedger::open(&state_root)
                .is_ok_and(|ledger| ledger.state.bus_identity.is_some())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            authority.calls().is_empty(),
            "retained migration must be skipped at activation"
        );
        let persist = Persist::open(bus_root.clone()).expect("reopen activated Bus");
        let forward = authorized_request_body(&request, "forward-after-bus-activation");
        persist
            .write(ACTION_TOPIC, Priority::Default, None, Some(&forward))
            .expect("publish first forward migration");
        for _ in 0..200 {
            if authority.calls().len() >= 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        shutdown_tx.send(true).expect("request shutdown");
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("worker shutdown")
            .expect("worker task")
            .expect("clean worker shutdown");

        assert!(attempts.load(Ordering::SeqCst) >= 2);
        assert_eq!(
            authority.calls(),
            vec![
                "capture:vm-queued-during-outage",
                "stop:vm-queued-during-outage",
                "define-start:vm-queued-during-outage",
            ],
            "only the first forward action runs, once, and restores on failed stop"
        );
        let recovered = MigrationLedger::open(&state_root).expect("recover ledger");
        assert!(recovered.state.source_cursor.is_some());
        assert!(recovered.state.source_jobs.is_empty());
        assert_eq!(recovered.state.pending_commits.len(), 1);
        assert!(matches!(
            recovered.state.pending_commits[0].phase,
            PendingPhase::Waiting
        ));
    }

    #[test]
    fn migration_ledger_rejects_symlink_duplicate_keys_and_oversize_state() {
        use std::os::unix::fs::symlink;

        let symlink_tmp = tempfile::tempdir().unwrap();
        let outside = symlink_tmp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let linked = symlink_tmp.path().join("linked");
        symlink(&outside, &linked).unwrap();
        assert!(MigrationLedger::open(&linked).is_err());

        let duplicate_tmp = tempfile::tempdir().unwrap();
        fs::write(
            duplicate_tmp.path().join(MIGRATION_LEDGER_FILE),
            br#"{"schema_version":1,"schema_version":1,"source_cursor":null,"target_cursor":null,"committed_cursor":null,"failed_cursor":null,"source_jobs":[],"target_jobs":[],"ack_jobs":[],"pending_commits":[]}"#,
        )
        .unwrap();
        assert!(MigrationLedger::open(duplicate_tmp.path()).is_err());

        let oversize_tmp = tempfile::tempdir().unwrap();
        let file = fs::File::create(oversize_tmp.path().join(MIGRATION_LEDGER_FILE)).unwrap();
        file.set_len(MAX_MIGRATION_LEDGER_BYTES + 1).unwrap();
        assert!(MigrationLedger::open(oversize_tmp.path()).is_err());
    }
}
