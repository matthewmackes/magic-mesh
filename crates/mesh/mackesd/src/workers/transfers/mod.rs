//! TRANSFERS-1 — the `transfers` mackesd worker: the queue/ledger/verb/state-machine
//! spine of the Transfers surface (`docs/design/transfers-surface.md`).
//!
//! The Transfers surface is "the one place every byte that moves is born, tracked,
//! and completed." Per §9 the GUI is a renderer: lifecycle lives in the daemon, so
//! jobs survive shell restarts, run headless, and any node can host them. This
//! module is that daemon spine:
//!
//! * a typed [`TransferJob`] envelope (id / source / dest / [`Method`] / [`policy`]
//!   / [`state`]) — the one record every protocol lane rides (Q4);
//! * a **persistent [`Ledger`]** on the node-local store (Q11 — history + state
//!   survive a reboot);
//! * the [`TransferQueue`] engine — the five-state machine + the **parallel cap**
//!   (Q12), every mutation written straight through to the ledger;
//! * the typed [`TransferVerb`] set — `submit / cancel / pause / resume / list`
//!   (Q14) — with an inbox transport the CLI (`mackesd transfer …`) drives for §9
//!   CLI parity;
//! * the injectable [`LaneRunner`] seam the per-protocol lanes (TRANSFERS-2..6)
//!   implement — defaulted here to [`TransferLaneRunner`], which wires the
//!   TRANSFERS-2 HTTP lane and keeps the remaining lanes honestly gated (§7).
//!
//! [`policy`]: TransferJob::policy
//! [`state`]: TransferJob::state
//!
//! ## Rank
//!
//! `transfers` is a **Workstation-tier (rank 1)** worker, the sibling of
//! `pty_broker` (TERM-7) and `mesh_mount` (FILEMGR-5): a mesh feature fronted by a
//! desktop surface (the File Browser, Q1). It idles gracefully where unused — a
//! Lighthouse relay or an untouched headless box simply drains an empty inbox and
//! keeps an empty ledger. A **deliberate census entry** in `worker_role::WORKER_REGISTRY`
//! (the BUG-STORAGE-1 lesson — a worker absent from the census silently never runs).

#![cfg(feature = "async-services")]

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use mackes_mesh_types::cloud::{cloud_request_digest, CloudArmSigner, CloudArmedToken};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_collab_types::{
    CollabCommand, FileRef, FileRefId, FileReferences, SpaceId, TransferError, TransferErrorCode,
    TransferId, TransferJobV2, TransferKind, TransferLocation, TransferOperation, TransferPhase,
    TransferState as V2State,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::task::JoinHandle;

use super::{ShutdownToken, Worker};

pub mod destination;
pub mod job;
pub mod lane;
pub mod ledger;
pub mod queue;
pub mod sync_pair;
pub mod v2;
pub mod verb;

pub use destination::{
    destinations_from_state, discover_destinations, is_ad_hoc_endpoint, DestinationKind,
    TransferDestination,
};
pub use job::{Method, TransferJob, TransferPolicy, TransferState, Transition};
pub use lane::{
    GatedLaneRunner, HttpWgetLane, LaneOutcome, LaneRunner, MusicLibraryLane, NodeLane,
    ProgressSink, RsyncLane, TransferLaneRunner,
};
pub use ledger::{Ledger, V2Ledger, V2LedgerError};
pub use queue::{QueueError, TransferQueue};
pub use sync_pair::{SyncPair, SyncPairStore};
pub use v2::{
    project_queued_job, BoundFilesEndpoint, FilesCommitFailure, FilesCopyError, FilesCopyOutcome,
    FilesEndpointResolver, FilesEndpointRole, FilesObjectType, FilesResolveFailure,
    ResolvedFilesEndpoint, ResolvedTransferJobV2, TransferV2Identity, TransferV2ProjectionError,
    TransferV2ResolutionError,
};
pub use verb::{inbox_dir, take_verbs, write_verb, TransferV2Control, TransferVerb};

/// Default number of jobs run in parallel when the cap env is unset (Q12).
pub const DEFAULT_PARALLEL_CAP: usize = 3;

/// Env var that overrides the parallel cap (Q12 — "configurable cap").
pub const CAP_ENV: &str = "MDE_TRANSFERS_PARALLEL_CAP";

/// Inbox drain cadence — a submitted/paused job is picked up within this window.
pub const POLL: Duration = Duration::from_secs(2);
/// The existing Chat worker folds `event/notify/*`; transfer terminal events use
/// this source lane instead of creating a new notification surface.
pub const TRANSFER_NOTIFY_TOPIC: &str = "event/notify/transfers";
const FILE_REFERENCES_TOPIC_PREFIX: &str = "state/collab/file-references/";
const MAX_FILES_IDENTITY_TOPICS: usize = 4_096;
const MAX_FILES_IDENTITY_BODY_BYTES: usize = 1024 * 1024;
const MAX_V2_LEDGER_RECORD_BYTES: usize = 1024 * 1024;
const COLLAB_FILES_COMMIT_VERB: &str = "collab-command";
const FILES_PROJECTION_CONFIRM_TIMEOUT: Duration = Duration::from_secs(5);
const FILES_PROJECTION_CONFIRM_POLL: Duration = Duration::from_millis(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum V2OperationKind {
    Copy,
    Sync,
    Download,
    Upload,
    Scrape,
    Mirror,
    PublishClipboard,
}

impl From<&TransferOperation> for V2OperationKind {
    fn from(operation: &TransferOperation) -> Self {
        match operation {
            TransferOperation::Copy { .. } => Self::Copy,
            TransferOperation::Sync { .. } => Self::Sync,
            TransferOperation::Download => Self::Download,
            TransferOperation::Upload => Self::Upload,
            TransferOperation::Scrape { .. } => Self::Scrape,
            TransferOperation::Mirror { .. } => Self::Mirror,
            TransferOperation::PublishClipboard => Self::PublishClipboard,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum V2ExecutorAdmission {
    LocalFilesCopy,
    Blocked(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct V2ExecutorRegistryRow {
    kind: TransferKind,
    operation: V2OperationKind,
    admission: V2ExecutorAdmission,
}

const V2_EXECUTOR_REGISTRY: [V2ExecutorRegistryRow; 11] = [
    V2ExecutorRegistryRow {
        kind: TransferKind::Local,
        operation: V2OperationKind::Copy,
        admission: V2ExecutorAdmission::LocalFilesCopy,
    },
    V2ExecutorRegistryRow {
        kind: TransferKind::Mesh,
        operation: V2OperationKind::Copy,
        admission: V2ExecutorAdmission::Blocked(
            "authenticated mesh transport and remote acknowledgement provider is unavailable",
        ),
    },
    V2ExecutorRegistryRow {
        kind: TransferKind::Rsync,
        operation: V2OperationKind::Sync,
        admission: V2ExecutorAdmission::Blocked(
            "V2 rsync profile executor provider is unavailable",
        ),
    },
    V2ExecutorRegistryRow {
        kind: TransferKind::Sftp,
        operation: V2OperationKind::Copy,
        admission: V2ExecutorAdmission::Blocked("sealed SFTP executor provider is unavailable"),
    },
    V2ExecutorRegistryRow {
        kind: TransferKind::Sftp,
        operation: V2OperationKind::Download,
        admission: V2ExecutorAdmission::Blocked("sealed SFTP executor provider is unavailable"),
    },
    V2ExecutorRegistryRow {
        kind: TransferKind::Sftp,
        operation: V2OperationKind::Upload,
        admission: V2ExecutorAdmission::Blocked("sealed SFTP executor provider is unavailable"),
    },
    V2ExecutorRegistryRow {
        kind: TransferKind::Http,
        operation: V2OperationKind::Download,
        admission: V2ExecutorAdmission::Blocked("HTTP resource executor provider is unavailable"),
    },
    V2ExecutorRegistryRow {
        kind: TransferKind::Scrape,
        operation: V2OperationKind::Scrape,
        admission: V2ExecutorAdmission::Blocked(
            "browser scrape materialization provider is unavailable",
        ),
    },
    V2ExecutorRegistryRow {
        kind: TransferKind::Multipart,
        operation: V2OperationKind::Upload,
        admission: V2ExecutorAdmission::Blocked(
            "sealed multipart upload executor provider is unavailable",
        ),
    },
    V2ExecutorRegistryRow {
        kind: TransferKind::Recurring,
        operation: V2OperationKind::Mirror,
        admission: V2ExecutorAdmission::Blocked(
            "recurring mirror scheduler and executor provider is unavailable",
        ),
    },
    V2ExecutorRegistryRow {
        kind: TransferKind::Clipboard,
        operation: V2OperationKind::PublishClipboard,
        admission: V2ExecutorAdmission::Blocked(
            "clipboard Files publication executor provider is unavailable",
        ),
    },
];

fn v2_executor_admission(job: &TransferJobV2) -> V2ExecutorAdmission {
    let operation = V2OperationKind::from(&job.operation);
    V2_EXECUTOR_REGISTRY
        .iter()
        .find(|row| row.kind == job.kind && row.operation == operation)
        .map_or(
            V2ExecutorAdmission::Blocked("contract kind and operation pair is not executable"),
            |row| row.admission,
        )
}

/// Wall-clock milliseconds since the epoch (the ledger's timestamps + id seed).
#[must_use]
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// The node-LOCAL transfers store root.
///
/// `<MDE_HOME|MACKESD_HOME>/transfers`, or `/var/lib/mde/transfers` when neither is
/// set (mirrors [`crate::default_db_path`]). The CLI and the daemon both resolve this
/// so they share the ledger + inbox.
#[must_use]
pub fn default_store_root() -> PathBuf {
    if let Some(home) = crate::env_with_legacy_fallback("MDE_HOME", "MACKESD_HOME") {
        return PathBuf::from(home).join("transfers");
    }
    PathBuf::from("/var/lib/mde/transfers")
}

/// The configured parallel cap (>= 1): [`CAP_ENV`] if a valid positive integer,
/// else [`DEFAULT_PARALLEL_CAP`].
#[must_use]
pub fn default_cap() -> usize {
    std::env::var(CAP_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .map_or(DEFAULT_PARALLEL_CAP, |n| n.max(1))
}

/// Production bridge from opaque V2 Files identities to the existing
/// collaboration Files authority.
///
/// Identity and metadata come only from retained
/// `state/collab/file-references/<space>` projections. Paths are derived from
/// the projection's verified content hash into the existing Syncthing-backed
/// `collab/content/<prefix>/<sha256>` store; the opaque `FileRefId` and mesh
/// node token are never parsed as a host path or URL.
#[derive(Clone)]
struct CollabFilesResolver {
    bus_root: Option<PathBuf>,
    content_root: PathBuf,
    action_signer: Option<CloudArmSigner>,
    actor: Option<String>,
    projection_confirm_timeout: Duration,
    #[cfg(test)]
    fail_publications: Arc<std::sync::atomic::AtomicUsize>,
}

impl CollabFilesResolver {
    fn new(bus_root: Option<PathBuf>, content_root: PathBuf) -> Self {
        Self {
            bus_root,
            content_root,
            action_signer: crate::ipc::action_auth::production_action_signer().ok(),
            actor: std::env::var("HOSTNAME")
                .ok()
                .filter(|actor| !actor.trim().is_empty()),
            projection_confirm_timeout: FILES_PROJECTION_CONFIRM_TIMEOUT,
            #[cfg(test)]
            fail_publications: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    #[cfg(test)]
    fn with_commit_authority(
        mut self,
        signer: CloudArmSigner,
        actor: impl Into<String>,
        confirm_timeout: Duration,
    ) -> Self {
        self.action_signer = Some(signer);
        self.actor = Some(actor.into());
        self.projection_confirm_timeout = confirm_timeout;
        self
    }

    #[cfg(test)]
    fn fail_next_publications(self, count: usize) -> Self {
        self.fail_publications.store(count, Ordering::Release);
        self
    }

    fn reference(
        &self,
        object: FileRefId,
        mesh_node: Option<&str>,
    ) -> Result<(SpaceId, FileRef, u64), FilesResolveFailure> {
        let bus_root = self
            .bus_root
            .as_ref()
            .ok_or(FilesResolveFailure::Unavailable)?;
        let persist =
            Persist::open(bus_root.clone()).map_err(|_| FilesResolveFailure::RegistryFailure)?;
        let topics = persist
            .list_topics()
            .map_err(|_| FilesResolveFailure::RegistryFailure)?;
        if topics.len() > MAX_FILES_IDENTITY_TOPICS {
            return Err(FilesResolveFailure::RegistryFailure);
        }

        let mut found: Option<(SpaceId, FileRef, u64)> = None;
        for topic in topics
            .iter()
            .filter(|topic| topic.starts_with(FILE_REFERENCES_TOPIC_PREFIX))
        {
            let Some(space) = topic
                .strip_prefix(FILE_REFERENCES_TOPIC_PREFIX)
                .and_then(|space| space.parse::<mde_collab_types::SpaceId>().ok())
            else {
                continue;
            };
            let Some(message) = persist
                .read_latest(topic)
                .map_err(|_| FilesResolveFailure::RegistryFailure)?
            else {
                continue;
            };
            let Some(body) = message.body.as_deref() else {
                continue;
            };
            if body.len() > MAX_FILES_IDENTITY_BODY_BYTES {
                return Err(FilesResolveFailure::RegistryFailure);
            }
            let references: FileReferences =
                serde_json::from_str(body).map_err(|_| FilesResolveFailure::RegistryFailure)?;
            for row in references
                .files
                .into_iter()
                .filter(|row| row.file == object)
            {
                if mesh_node.is_some_and(|node| row.linked_by.as_str() != node) {
                    continue;
                }
                let generation = u64::try_from(row.linked_unix_ms)
                    .ok()
                    .filter(|generation| *generation > 0)
                    .ok_or(FilesResolveFailure::RegistryFailure)?;
                match found {
                    None => found = Some((space, row.reference, generation)),
                    Some(_) => return Err(FilesResolveFailure::RegistryFailure),
                }
            }
        }
        found.ok_or(FilesResolveFailure::Unavailable)
    }

    fn commit_content_addressed(
        &self,
        admitted: &ResolvedTransferJobV2,
        staged_path: &std::path::Path,
        outcome: &FilesCopyOutcome,
    ) -> Result<(), FilesCommitFailure> {
        let signer = self
            .action_signer
            .as_ref()
            .ok_or(FilesCommitFailure::MutationUnsupported)?;
        let actor = self
            .actor
            .as_deref()
            .ok_or(FilesCommitFailure::MutationUnsupported)?;
        let (object, mesh_node) = match admitted.destination().identity() {
            TransferLocation::Local { object } => (*object, None),
            TransferLocation::Mesh { node, object } => (*object, Some(node.as_str())),
            _ => return Err(FilesCommitFailure::MutationUnsupported),
        };
        let (space, current, generation) = self
            .reference(object, mesh_node)
            .map_err(|_| FilesCommitFailure::ConcurrentDestination)?;
        let expected = admitted.destination_record();
        if generation != expected.generation
            || current.sha256_hex != expected.sha256_hex
            || current.size != expected.size_bytes
        {
            return Err(FilesCommitFailure::ConcurrentDestination);
        }
        if outcome.bytes_copied != admitted.source().size_bytes()
            || outcome.sha256_hex != admitted.source().sha256_hex()
        {
            return Err(FilesCommitFailure::Filesystem);
        }

        let canonical_root = std::fs::canonicalize(&self.content_root)
            .map_err(|_| FilesCommitFailure::Filesystem)?;
        verify_staged_copy(staged_path, &canonical_root, outcome)?;
        let target = install_content_address(
            staged_path,
            &canonical_root,
            &outcome.sha256_hex,
            outcome.bytes_copied,
        )?;
        // The exact generation is already committed: this is the idempotent
        // retry after a prior metadata publication succeeded but its ack was
        // lost. Content verification above still runs before success.
        if current.sha256_hex == outcome.sha256_hex && current.size == outcome.bytes_copied {
            let _ = std::fs::remove_file(staged_path);
            return Ok(());
        }

        let replacement = FileRef {
            name: current.name,
            size: outcome.bytes_copied,
            sha256_hex: outcome.sha256_hex.clone(),
            mime: current.mime,
        };
        let command = CollabCommand::CommitFileGeneration {
            space,
            file: object,
            expected_generation: i64::try_from(generation)
                .map_err(|_| FilesCommitFailure::ConcurrentDestination)?,
            expected_sha256_hex: expected.sha256_hex.clone(),
            expected_size: expected.size_bytes,
            reference: replacement.clone(),
        };
        self.publish_commit_command(signer, actor, admitted, &command)?;
        self.await_generation_projection(
            space,
            object,
            i64::try_from(generation).map_err(|_| FilesCommitFailure::ConcurrentDestination)?,
            expected,
            &replacement,
        )?;
        // `target` is deliberately retained even when projection publication
        // fails: an unreferenced correctly named hash object is safe and makes
        // retry idempotent. The staging inode is no longer needed.
        let _ = target;
        let _ = std::fs::remove_file(staged_path);
        Ok(())
    }

    fn publish_commit_command(
        &self,
        signer: &CloudArmSigner,
        actor: &str,
        admitted: &ResolvedTransferJobV2,
        command: &CollabCommand,
    ) -> Result<(), FilesCommitFailure> {
        #[cfg(test)]
        if self
            .fail_publications
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err(FilesCommitFailure::Publication);
        }

        let bus_root = self
            .bus_root
            .as_ref()
            .ok_or(FilesCommitFailure::Publication)?;
        let mut unsigned =
            serde_json::to_value(command).map_err(|_| FilesCommitFailure::Publication)?;
        unsigned
            .as_object_mut()
            .ok_or(FilesCommitFailure::Publication)?
            .insert(
                "schema_version".to_string(),
                serde_json::Value::from(crate::ipc::action_auth::ACTION_SCHEMA_VERSION),
            );
        let unsigned =
            serde_json::to_string(&unsigned).map_err(|_| FilesCommitFailure::Publication)?;
        let auth_now = i64::try_from(now_ms()).unwrap_or(i64::MAX);
        let nonce = format!(
            "files-commit-{}-{}-{auth_now}",
            admitted.job().transfer,
            admitted.job().progress.attempt
        );
        let token = CloudArmedToken::mint(
            signer,
            &nonce,
            auth_now.saturating_add(crate::ipc::action_auth::MAX_AUTH_TTL_MS),
            COLLAB_FILES_COMMIT_VERB,
            actor,
            command.verb(),
            &cloud_request_digest(&unsigned).map_err(|_| FilesCommitFailure::Publication)?,
        )
        .encode();
        let mut body: serde_json::Value =
            serde_json::from_str(&unsigned).map_err(|_| FilesCommitFailure::Publication)?;
        body.as_object_mut()
            .ok_or(FilesCommitFailure::Publication)?
            .insert("armed_token".to_string(), serde_json::Value::String(token));
        let body = serde_json::to_string(&body).map_err(|_| FilesCommitFailure::Publication)?;
        Persist::open(bus_root.clone())
            .and_then(|persist| {
                persist.write(
                    &mde_collab_types::topics::command_topic(command.verb()),
                    Priority::Default,
                    None,
                    Some(&body),
                )
            })
            .map(|_| ())
            .map_err(|_| FilesCommitFailure::Publication)
    }

    fn await_generation_projection(
        &self,
        space: SpaceId,
        file: FileRefId,
        expected_generation: i64,
        expected: &ResolvedFilesEndpoint,
        replacement: &FileRef,
    ) -> Result<(), FilesCommitFailure> {
        let bus_root = self
            .bus_root
            .as_ref()
            .ok_or(FilesCommitFailure::PublicationUnconfirmed)?;
        let deadline = Instant::now()
            .checked_add(self.projection_confirm_timeout)
            .ok_or(FilesCommitFailure::PublicationUnconfirmed)?;
        let topic = mde_collab_types::topics::space_state_topic(
            mde_collab_types::topics::projection::FILE_REFERENCES,
            space,
        );
        loop {
            if let Ok(persist) = Persist::open(bus_root.clone()) {
                if let Ok(Some(message)) = persist.read_latest(&topic) {
                    if let Some(body) = message.body.as_deref() {
                        if let Ok(rows) = serde_json::from_str::<FileReferences>(body) {
                            match rows.files.iter().find(|row| row.file == file) {
                                Some(row)
                                    if row.reference == *replacement
                                        && row.linked_unix_ms > expected_generation =>
                                {
                                    return Ok(());
                                }
                                Some(row)
                                    if row.reference.sha256_hex == expected.sha256_hex
                                        && row.reference.size == expected.size_bytes
                                        && row.linked_unix_ms == expected_generation => {}
                                Some(_) | None => {
                                    return Err(FilesCommitFailure::ConcurrentDestination);
                                }
                            }
                        }
                    }
                }
            }
            if Instant::now() >= deadline {
                return Err(FilesCommitFailure::PublicationUnconfirmed);
            }
            std::thread::sleep(FILES_PROJECTION_CONFIRM_POLL);
        }
    }
}

fn verify_staged_copy(
    staged_path: &std::path::Path,
    canonical_root: &std::path::Path,
    outcome: &FilesCopyOutcome,
) -> Result<(), FilesCommitFailure> {
    let metadata =
        std::fs::symlink_metadata(staged_path).map_err(|_| FilesCommitFailure::Filesystem)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() != outcome.bytes_copied
    {
        return Err(FilesCommitFailure::Filesystem);
    }
    let canonical =
        std::fs::canonicalize(staged_path).map_err(|_| FilesCommitFailure::Filesystem)?;
    if canonical != staged_path || !canonical.starts_with(canonical_root) {
        return Err(FilesCommitFailure::Filesystem);
    }
    verify_content_file(staged_path, &outcome.sha256_hex, outcome.bytes_copied)
}

fn install_content_address(
    staged_path: &std::path::Path,
    canonical_root: &std::path::Path,
    sha256_hex: &str,
    size_bytes: u64,
) -> Result<PathBuf, FilesCommitFailure> {
    let prefix = sha256_hex.get(..2).ok_or(FilesCommitFailure::Filesystem)?;
    let shard = canonical_root.join(prefix);
    match std::fs::symlink_metadata(&shard) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err(FilesCommitFailure::Filesystem),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match std::fs::create_dir(&shard) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let metadata = std::fs::symlink_metadata(&shard)
                        .map_err(|_| FilesCommitFailure::Filesystem)?;
                    if !metadata.is_dir() || metadata.file_type().is_symlink() {
                        return Err(FilesCommitFailure::Filesystem);
                    }
                }
                Err(_) => return Err(FilesCommitFailure::Filesystem),
            }
        }
        Err(_) => return Err(FilesCommitFailure::Filesystem),
    }
    let target = shard.join(sha256_hex);
    match std::fs::hard_link(staged_path, &target) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(FilesCommitFailure::Filesystem),
    }
    verify_content_file(&target, sha256_hex, size_bytes)?;
    std::fs::File::open(&shard)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| FilesCommitFailure::Filesystem)?;
    Ok(target)
}

fn verify_content_file(
    path: &std::path::Path,
    expected_sha256_hex: &str,
    expected_size: u64,
) -> Result<(), FilesCommitFailure> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(0o400000);
    }
    let mut file = options
        .open(path)
        .map_err(|_| FilesCommitFailure::Filesystem)?;
    let metadata = file
        .metadata()
        .map_err(|_| FilesCommitFailure::Filesystem)?;
    if !metadata.is_file() || metadata.len() != expected_size {
        return Err(FilesCommitFailure::Filesystem);
    }
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| FilesCommitFailure::Filesystem)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    if format!("{:x}", digest.finalize()) != expected_sha256_hex {
        return Err(FilesCommitFailure::Filesystem);
    }
    Ok(())
}

impl FilesEndpointResolver for CollabFilesResolver {
    fn resolve(
        &self,
        identity: &TransferLocation,
        role: FilesEndpointRole,
    ) -> Result<ResolvedFilesEndpoint, FilesResolveFailure> {
        let (object, mesh_node) = match identity {
            TransferLocation::Local { object } => (*object, None),
            TransferLocation::Mesh { node, object } => (*object, Some(node.as_str())),
            _ => return Err(FilesResolveFailure::RegistryFailure),
        };
        let (_space, reference, generation) = self.reference(object, mesh_node)?;
        if role == FilesEndpointRole::Destination
            && (self.action_signer.is_none() || self.actor.is_none())
        {
            return Err(FilesResolveFailure::MutationUnsupported);
        }
        let prefix = reference
            .sha256_hex
            .get(..2)
            .ok_or(FilesResolveFailure::RegistryFailure)?;
        let canonical_root = std::fs::canonicalize(&self.content_root)
            .map_err(|_| FilesResolveFailure::Unavailable)?;
        let relative_path = PathBuf::from(prefix).join(&reference.sha256_hex);
        let path = canonical_root.join(&relative_path);
        let available = std::fs::symlink_metadata(&path)
            .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink());
        let writable = role != FilesEndpointRole::Destination
            || path
                .parent()
                .and_then(|parent| std::fs::metadata(parent).ok())
                .is_some_and(|metadata| metadata.is_dir() && !metadata.permissions().readonly());
        Ok(ResolvedFilesEndpoint {
            identity: identity.clone(),
            canonical_root,
            relative_path,
            generation,
            sha256_hex: reference.sha256_hex,
            size_bytes: reference.size,
            object_type: FilesObjectType::RegularFile,
            available,
            readable: available,
            writable,
        })
    }

    fn commit_staged_copy(
        &self,
        admitted: &ResolvedTransferJobV2,
        staged_path: &std::path::Path,
        outcome: &FilesCopyOutcome,
    ) -> Result<(), FilesCommitFailure> {
        self.commit_content_addressed(admitted, staged_path, outcome)
    }
}

/// The `transfers` worker — drives the queue: drains the inbox, reaps finished lane
/// tasks, and fills up to the cap each tick.
pub struct TransfersWorker {
    store_root: PathBuf,
    bus_root: Option<PathBuf>,
    cap: usize,
    lane: Arc<dyn LaneRunner>,
    files_resolver: Arc<dyn FilesEndpointResolver>,
    poll: Duration,
}

impl TransfersWorker {
    /// Production constructor: the node-local store, the env cap, and the method
    /// dispatcher (HTTP wired; future lanes still honestly gated).
    #[must_use]
    pub fn new(store_root: PathBuf) -> Self {
        let bus_root =
            mde_bus::default_data_dir().or_else(|| Some(PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)));
        let files_resolver = Arc::new(CollabFilesResolver::new(
            bus_root.clone(),
            crate::default_qnm_shared_root()
                .join("collab")
                .join("content"),
        ));
        Self {
            store_root,
            bus_root,
            cap: default_cap(),
            lane: Arc::new(TransferLaneRunner),
            files_resolver,
            poll: POLL,
        }
    }

    /// Override the parallel cap (tests + a future config plumb).
    #[must_use]
    pub fn with_cap(mut self, cap: usize) -> Self {
        self.cap = cap.max(1);
        self
    }

    /// Inject the lane runner (the TRANSFERS-2..6 seam; tests supply a fake).
    #[must_use]
    pub fn with_lane(mut self, lane: Arc<dyn LaneRunner>) -> Self {
        self.lane = lane;
        self
    }

    /// Inject the canonical Files identity resolver (tests and alternate
    /// authority transports use the same typed seam).
    #[must_use]
    pub fn with_files_resolver(mut self, resolver: Arc<dyn FilesEndpointResolver>) -> Self {
        self.files_resolver = resolver;
        self
    }

    /// Override the Bus root used for terminal notification tests.
    #[must_use]
    pub fn with_bus_root(mut self, bus_root: Option<PathBuf>) -> Self {
        self.bus_root = bus_root;
        self
    }

    /// Override the poll cadence (tests use a short value).
    #[must_use]
    pub const fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    /// Open the live engine (the queue over the ledger + the empty task table).
    ///
    /// # Errors
    /// Fails if the ledger directory can't be opened.
    fn engine(&self) -> std::io::Result<Engine> {
        Ok(Engine {
            queue: TransferQueue::open(&self.store_root, self.cap)?,
            v2_ledger: V2Ledger::open(&self.store_root)?,
            sync_pairs: SyncPairStore::open(&self.store_root)?,
            tasks: HashMap::new(),
            v2_tasks: HashMap::new(),
            lane: Arc::clone(&self.lane),
            files_resolver: Arc::clone(&self.files_resolver),
            v2_ledger_lock: Arc::new(Mutex::new(())),
            cap: self.cap,
            store_root: self.store_root.clone(),
            notify: self.bus_root.clone().map(TransferNotifier::new),
        })
    }
}

#[async_trait::async_trait]
impl Worker for TransfersWorker {
    fn name(&self) -> &'static str {
        "transfers"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let mut engine = self.engine()?;
        tracing::info!(
            target: "mackesd::transfers",
            store = %self.store_root.display(), cap = self.cap,
            "transfers worker up (queue/ledger/verb spine; http lane wired, remaining lanes honestly gated)",
        );
        loop {
            engine.tick().await;
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(self.poll) => {}
            }
        }
    }
}

/// The live run state: the open queue, the in-flight lane tasks, and the seam.
struct Engine {
    queue: TransferQueue,
    v2_ledger: V2Ledger,
    sync_pairs: SyncPairStore,
    tasks: HashMap<String, JoinHandle<LaneOutcome>>,
    v2_tasks: HashMap<TransferId, V2Task>,
    lane: Arc<dyn LaneRunner>,
    files_resolver: Arc<dyn FilesEndpointResolver>,
    v2_ledger_lock: Arc<Mutex<()>>,
    cap: usize,
    store_root: PathBuf,
    notify: Option<TransferNotifier>,
}

struct V2Task {
    canceled: Arc<AtomicBool>,
    handle: JoinHandle<V2TaskResult>,
}

enum V2TaskResult {
    Terminal {
        job: TransferJobV2,
        checksum_sha256: Option<String>,
    },
    Superseded,
}

impl Engine {
    /// One scheduler pass: apply inbox verbs, reap finished tasks, fill to the cap.
    async fn tick(&mut self) {
        self.drain_inbox();
        self.schedule_sync_pairs_at(now_ms());
        self.reap().await;
        self.reap_v2().await;
        self.fill_v2();
        self.fill();
    }

    /// Fire every due saved sync pair by enqueueing a normal rsync job.
    fn schedule_sync_pairs_at(&mut self, now: u64) {
        for pair in self.sync_pairs.load_all() {
            if !pair.due_at(now) {
                continue;
            }
            let id = pair.id.clone();
            let job = pair.to_job();
            let job_id = job.id.clone();
            match self.queue.submit(job) {
                Ok(_) => {
                    if let Err(e) = self.sync_pairs.mark_fired(&id, now) {
                        tracing::warn!(
                            target: "mackesd::transfers",
                            pair = %id, job = %job_id, error = %e,
                            "sync pair fired but last_fired stamp failed"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        target: "mackesd::transfers",
                        pair = %id, error = %e,
                        "sync pair enqueue failed"
                    );
                }
            }
        }
    }

    /// Apply every pending inbox verb (the daemon is the single ledger writer).
    fn drain_inbox(&mut self) {
        for verb in take_verbs(&self.store_root) {
            match verb {
                TransferVerb::Submit(job) => {
                    let id = job.id.clone();
                    if let Err(e) = self.queue.submit(job) {
                        tracing::warn!(target: "mackesd::transfers", id = %id, error = %e, "submit failed");
                    }
                }
                TransferVerb::SubmitV2(job) => {
                    let id = job.transfer;
                    let result = match self.v2_ledger_lock.lock() {
                        Ok(_guard) => self
                            .v2_ledger
                            .submit(job)
                            .map_err(|error| error.to_string()),
                        Err(_) => Err("V2 ledger lock poisoned".to_string()),
                    };
                    match result {
                        Ok(()) => tracing::info!(
                            target: "mackesd::transfers",
                            transfer = %id,
                            "admitted V2 transfer for typed Files resolution"
                        ),
                        Err(error) => tracing::warn!(
                            target: "mackesd::transfers",
                            transfer = %id,
                            error,
                            "V2 transfer admission refused"
                        ),
                    }
                }
                TransferVerb::Cancel(id) => {
                    self.abort_task(&id);
                    let res = self.queue.cancel(&id);
                    Self::log_verb("cancel", &id, res);
                }
                TransferVerb::Pause(id) => {
                    self.abort_task(&id);
                    let res = self.queue.pause(&id);
                    Self::log_verb("pause", &id, res);
                }
                TransferVerb::Resume(id) => {
                    let res = self.queue.resume(&id);
                    Self::log_verb("resume", &id, res);
                }
                TransferVerb::ControlV2(command) => {
                    let result = self
                        .v2_ledger_lock
                        .lock()
                        .map_err(|_| "V2 ledger lock poisoned".to_string())
                        .and_then(|_guard| {
                            self.v2_ledger
                                .apply_control(
                                    command.transfer,
                                    command.control,
                                    command.updated_unix_ms,
                                )
                                .map_err(|error| error.to_string())
                        });
                    if let Err(error) = result {
                        tracing::warn!(
                            target: "mackesd::transfers",
                            transfer = %command.transfer,
                            error = %error,
                            "V2 transfer control refused"
                        );
                    } else if matches!(
                        command.control,
                        mde_collab_types::TransferControlV2::Pause
                            | mde_collab_types::TransferControlV2::Cancel
                    ) {
                        if let Some(task) = self.v2_tasks.get(&command.transfer) {
                            task.canceled.store(true, Ordering::Release);
                        }
                    }
                }
                TransferVerb::SaveSyncPair(pair) => {
                    let id = pair.id.clone();
                    if let Err(e) = self.sync_pairs.upsert(&pair) {
                        tracing::warn!(target: "mackesd::transfers", pair = %id, error = %e, "save sync pair failed");
                    }
                }
                TransferVerb::RemoveSyncPair(id) => {
                    if let Err(e) = self.sync_pairs.remove(&id) {
                        tracing::warn!(target: "mackesd::transfers", pair = %id, error = %e, "remove sync pair failed");
                    }
                }
                // `list` is a pure read served off the ledger by the caller — the
                // daemon has nothing to do for it.
                TransferVerb::List => {}
            }
        }
    }

    /// Abort + forget a job's in-flight lane task (tokio abort → the lane's child
    /// process is killed on drop, so a cancel/pause stops a running transfer).
    fn abort_task(&mut self, id: &str) {
        if let Some(handle) = self.tasks.remove(id) {
            handle.abort();
        }
    }

    /// Log a verb's typed outcome (an illegal/not-found refusal is honest, not
    /// silent).
    fn log_verb(verb: &str, id: &str, res: Result<(), QueueError>) {
        match res {
            Ok(()) => tracing::info!(target: "mackesd::transfers", verb, id, "applied"),
            Err(e) => tracing::info!(target: "mackesd::transfers", verb, id, error = %e, "refused"),
        }
    }

    /// Apply the outcome of every finished lane task to the ledger.
    async fn reap(&mut self) {
        let finished: Vec<String> = self
            .tasks
            .iter()
            .filter(|(_, h)| h.is_finished())
            .map(|(id, _)| id.clone())
            .collect();
        let mut terminal = Vec::new();
        for id in finished {
            let Some(handle) = self.tasks.remove(&id) else {
                continue;
            };
            // A JoinError means the task was aborted (a cancel/pause already moved the
            // job, so `complete` no-ops) or it panicked — either way, an honest fail.
            let outcome = handle
                .await
                .unwrap_or_else(|_| LaneOutcome::failed("the transfer lane task ended abnormally"));
            if let Err(e) = self.queue.complete(&id, &outcome) {
                tracing::warn!(target: "mackesd::transfers", id = %id, error = %e, "complete failed");
            } else if let Some(job) = self.queue.get(&id) {
                if job.state.is_terminal() {
                    terminal.push(job);
                }
            }
        }
        if let (Some(notify), false) = (&self.notify, terminal.is_empty()) {
            notify.emit_terminal_batch(&terminal);
        }
    }

    /// Reap strict V2 executor attempts and publish their durable terminal row.
    async fn reap_v2(&mut self) {
        let finished: Vec<TransferId> = self
            .v2_tasks
            .iter()
            .filter(|(_, task)| task.handle.is_finished())
            .map(|(id, _)| *id)
            .collect();
        for id in finished {
            let Some(task) = self.v2_tasks.remove(&id) else {
                continue;
            };
            match task.handle.await {
                Ok(V2TaskResult::Terminal {
                    job,
                    checksum_sha256,
                }) => {
                    if let Some(notify) = &self.notify {
                        notify.emit_v2_terminal(&job, checksum_sha256.as_deref());
                    }
                }
                Ok(V2TaskResult::Superseded) => {}
                Err(_) => {
                    let error = typed_transfer_error(
                        TransferErrorCode::Internal,
                        true,
                        "executor task ended abnormally",
                    );
                    if let Some(job) =
                        finish_v2_failed(&self.v2_ledger, &self.v2_ledger_lock, id, None, error)
                    {
                        if let Some(notify) = &self.notify {
                            notify.emit_v2_terminal(&job, None);
                        }
                    }
                }
            }
        }
    }

    /// Claim queued V2 jobs and start only the typed Local/Mesh Files lane.
    fn fill_v2(&mut self) {
        if self.tasks.len() + self.v2_tasks.len() >= self.cap {
            return;
        }
        for queued in self.v2_ledger.load_all() {
            if self.tasks.len() + self.v2_tasks.len() >= self.cap {
                break;
            }
            if queued.state != V2State::Queued || self.v2_tasks.contains_key(&queued.transfer) {
                continue;
            }
            let Some(claimed) =
                claim_v2_job(&self.v2_ledger, &self.v2_ledger_lock, queued.transfer)
            else {
                continue;
            };
            let id = claimed.transfer;
            let canceled = Arc::new(AtomicBool::new(false));
            let task_canceled = Arc::clone(&canceled);
            let ledger = self.v2_ledger.clone();
            let ledger_lock = Arc::clone(&self.v2_ledger_lock);
            let resolver = Arc::clone(&self.files_resolver);
            let handle = tokio::task::spawn_blocking(move || {
                run_v2_copy_attempt(ledger, ledger_lock, resolver, claimed, task_canceled)
            });
            self.v2_tasks.insert(id, V2Task { canceled, handle });
        }
    }

    /// Claim + spawn Queued jobs until the cap is reached or the queue is empty.
    fn fill(&mut self) {
        while self.tasks.len() + self.v2_tasks.len() < self.cap {
            let Some(job) = self.queue.claim_next() else {
                break;
            };
            let lane = Arc::clone(&self.lane);
            let queue = self.queue.clone();
            let progress_id = job.id.clone();
            let progress = ProgressSink::new(move |pct| {
                if let Err(e) = queue.set_progress(&progress_id, pct) {
                    tracing::warn!(
                        target: "mackesd::transfers",
                        id = %progress_id, error = %e,
                        "progress update failed"
                    );
                }
            });
            let running = job.clone();
            let handle = tokio::spawn(async move { lane.run(&running, progress).await });
            self.tasks.insert(job.id, handle);
        }
    }
}

fn next_v2_update(current: u64) -> Option<u64> {
    let wall = now_ms();
    Some(wall.max(current.checked_add(1)?))
}

fn persist_v2_runtime_job(ledger: &V2Ledger, job: &TransferJobV2) -> std::io::Result<()> {
    job.validate()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let body = serde_json::to_vec_pretty(job)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if body.len() > MAX_V2_LEDGER_RECORD_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "V2 ledger record exceeds the byte limit",
        ));
    }
    let dir = ledger.dir();
    let dir_metadata = std::fs::symlink_metadata(dir)?;
    if dir_metadata.file_type().is_symlink() || !dir_metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "V2 ledger directory is unsafe",
        ));
    }
    let target = dir.join(format!("{}.json", job.transfer));
    if let Ok(metadata) = std::fs::symlink_metadata(&target) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "V2 ledger target is unsafe",
            ));
        }
    }
    let temporary = dir.join(format!(
        ".{}.runtime-{}-{}.tmp",
        job.transfer,
        job.progress.attempt,
        std::process::id()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600).custom_flags(0o400000);
    }
    let mut file = options.open(&temporary)?;
    let write_result = (|| {
        file.write_all(&body)?;
        file.sync_all()?;
        drop(file);
        if let Ok(metadata) = std::fs::symlink_metadata(&target) {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "V2 ledger target changed to an unsafe object",
                ));
            }
        }
        std::fs::rename(&temporary, &target)?;
        std::fs::File::open(dir)?.sync_all()
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    write_result
}

fn claim_v2_job(
    ledger: &V2Ledger,
    ledger_lock: &Mutex<()>,
    id: TransferId,
) -> Option<TransferJobV2> {
    let _guard = ledger_lock.lock().ok()?;
    let mut job = ledger.get(id)?;
    if job.state != V2State::Queued || job.progress.phase != TransferPhase::Queued {
        return None;
    }
    job.progress.attempt = job.progress.attempt.checked_add(1)?;
    job.state = V2State::Active;
    job.progress.phase = TransferPhase::Resolving;
    job.progress.bytes_done = 0;
    job.progress.total_bytes = None;
    job.progress.bytes_per_second = None;
    job.progress.error = None;
    job.updated_unix_ms = next_v2_update(job.updated_unix_ms)?;
    persist_v2_runtime_job(ledger, &job).ok()?;
    Some(job)
}

fn update_active_v2(
    ledger: &V2Ledger,
    ledger_lock: &Mutex<()>,
    id: TransferId,
    attempt: u16,
    update: impl FnOnce(&mut TransferJobV2),
) -> Option<TransferJobV2> {
    let _guard = ledger_lock.lock().ok()?;
    let mut job = ledger.get(id)?;
    if job.state != V2State::Active || job.progress.attempt != attempt {
        return None;
    }
    update(&mut job);
    job.updated_unix_ms = next_v2_update(job.updated_unix_ms)?;
    persist_v2_runtime_job(ledger, &job).ok()?;
    Some(job)
}

fn finish_v2_failed(
    ledger: &V2Ledger,
    ledger_lock: &Mutex<()>,
    id: TransferId,
    expected_attempt: Option<u16>,
    error: TransferError,
) -> Option<TransferJobV2> {
    let _guard = ledger_lock.lock().ok()?;
    let mut job = ledger.get(id)?;
    if job.state != V2State::Active
        || expected_attempt.is_some_and(|attempt| attempt != job.progress.attempt)
    {
        return None;
    }
    job.state = V2State::Failed;
    job.progress.phase = TransferPhase::Failed;
    job.progress.bytes_per_second = None;
    job.progress.error = Some(error);
    job.updated_unix_ms = next_v2_update(job.updated_unix_ms)?;
    persist_v2_runtime_job(ledger, &job).ok()?;
    Some(job)
}

fn typed_transfer_error(
    code: TransferErrorCode,
    retryable: bool,
    detail: &'static str,
) -> TransferError {
    TransferError::new(code, retryable, Some(detail.to_string()))
        .expect("static transfer error detail satisfies the shared contract")
}

fn resolution_failure(error: &TransferV2ResolutionError) -> TransferError {
    match error {
        TransferV2ResolutionError::UnsupportedKind(_)
        | TransferV2ResolutionError::UnsupportedOperation
        | TransferV2ResolutionError::NonFilesIdentity(_)
        | TransferV2ResolutionError::Resolver {
            failure: FilesResolveFailure::MutationUnsupported,
            ..
        } => typed_transfer_error(
            TransferErrorCode::Unsupported,
            false,
            "executor protocol is not implemented",
        ),
        TransferV2ResolutionError::Resolver {
            failure: FilesResolveFailure::PermissionDenied,
            ..
        }
        | TransferV2ResolutionError::AccessDenied(_) => typed_transfer_error(
            TransferErrorCode::PermissionDenied,
            false,
            "Files authority denied endpoint access",
        ),
        TransferV2ResolutionError::MetadataMismatch {
            field: "sha256", ..
        }
        | TransferV2ResolutionError::ChecksumMismatch => typed_transfer_error(
            TransferErrorCode::ChecksumMismatch,
            false,
            "Files generation checksum mismatch",
        ),
        TransferV2ResolutionError::InvalidJob(_)
        | TransferV2ResolutionError::NotQueued
        | TransferV2ResolutionError::IdentityMismatch(_)
        | TransferV2ResolutionError::MetadataMismatch { .. }
        | TransferV2ResolutionError::IncompatibleObjectType(_)
        | TransferV2ResolutionError::SameCanonicalObject => typed_transfer_error(
            TransferErrorCode::InvalidRequest,
            false,
            "Files endpoint admission rejected",
        ),
        TransferV2ResolutionError::Resolver { .. }
        | TransferV2ResolutionError::Unavailable(_)
        | TransferV2ResolutionError::UnsafePath(_)
        | TransferV2ResolutionError::MetadataUnavailable(_)
        | TransferV2ResolutionError::StaleResolution(_) => typed_transfer_error(
            TransferErrorCode::ReferenceUnavailable,
            true,
            "Files generation is unavailable or stale",
        ),
    }
}

fn copy_failure(error: &FilesCopyError) -> TransferError {
    match error {
        FilesCopyError::Revalidation(error) => resolution_failure(error),
        FilesCopyError::Canceled => typed_transfer_error(
            TransferErrorCode::Canceled,
            false,
            "transfer attempt canceled",
        ),
        FilesCopyError::SourceChanged => typed_transfer_error(
            TransferErrorCode::ChecksumMismatch,
            false,
            "source generation changed during copy",
        ),
        FilesCopyError::Commit(FilesCommitFailure::MutationUnsupported) => typed_transfer_error(
            TransferErrorCode::Unsupported,
            false,
            "Files destination mutation authority is unavailable",
        ),
        FilesCopyError::Commit(FilesCommitFailure::ConcurrentDestination) => typed_transfer_error(
            TransferErrorCode::ReferenceUnavailable,
            true,
            "destination generation changed before commit",
        ),
        FilesCopyError::Commit(FilesCommitFailure::Filesystem) => typed_transfer_error(
            TransferErrorCode::Internal,
            true,
            "safe Files content commit failed",
        ),
        FilesCopyError::Commit(
            FilesCommitFailure::Publication | FilesCommitFailure::PublicationUnconfirmed,
        ) => typed_transfer_error(
            TransferErrorCode::Internal,
            true,
            "Files metadata publication failed or is unconfirmed",
        ),
    }
}

fn run_v2_copy_attempt(
    ledger: V2Ledger,
    ledger_lock: Arc<Mutex<()>>,
    resolver: Arc<dyn FilesEndpointResolver>,
    claimed: TransferJobV2,
    canceled: Arc<AtomicBool>,
) -> V2TaskResult {
    let attempt = claimed.progress.attempt;
    let id = claimed.transfer;
    if let V2ExecutorAdmission::Blocked(provider) = v2_executor_admission(&claimed) {
        return finish_v2_failed(
            &ledger,
            &ledger_lock,
            id,
            Some(attempt),
            typed_transfer_error(TransferErrorCode::Unsupported, false, provider),
        )
        .map_or(V2TaskResult::Superseded, |job| V2TaskResult::Terminal {
            job,
            checksum_sha256: None,
        });
    }
    let admitted = match v2::resolve_for_execution(claimed, resolver.as_ref()) {
        Ok(admitted) => admitted,
        Err(error) => {
            return finish_v2_failed(
                &ledger,
                &ledger_lock,
                id,
                Some(attempt),
                resolution_failure(&error),
            )
            .map_or(V2TaskResult::Superseded, |job| V2TaskResult::Terminal {
                job,
                checksum_sha256: None,
            });
        }
    };

    if update_active_v2(&ledger, &ledger_lock, id, attempt, |job| {
        job.progress.phase = TransferPhase::Transferring;
        job.progress.total_bytes = Some(admitted.source().size_bytes());
    })
    .is_none()
    {
        return V2TaskResult::Superseded;
    }

    let started = Instant::now();
    match v2::execute_local_mesh_copy(&admitted, resolver.as_ref(), &canceled) {
        Ok(outcome) => {
            let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let rate = (elapsed_ms > 0 && outcome.bytes_copied > 0).then(|| {
                outcome
                    .bytes_copied
                    .saturating_mul(1_000)
                    .checked_div(elapsed_ms)
                    .unwrap_or(1)
                    .clamp(1, 1_000_000_000_000)
            });
            update_active_v2(&ledger, &ledger_lock, id, attempt, |job| {
                job.state = V2State::Completed;
                job.progress.phase = TransferPhase::Completed;
                job.progress.bytes_done = outcome.bytes_copied;
                job.progress.total_bytes = Some(outcome.bytes_copied);
                job.progress.bytes_per_second = rate;
                job.progress.error = None;
            })
            .map_or(V2TaskResult::Superseded, |job| V2TaskResult::Terminal {
                job,
                checksum_sha256: Some(outcome.sha256_hex),
            })
        }
        Err(error) => finish_v2_failed(
            &ledger,
            &ledger_lock,
            id,
            Some(attempt),
            copy_failure(&error),
        )
        .map_or(V2TaskResult::Superseded, |job| V2TaskResult::Terminal {
            job,
            checksum_sha256: None,
        }),
    }
}

#[derive(Clone)]
struct TransferNotifier {
    bus_root: PathBuf,
}

impl TransferNotifier {
    fn new(bus_root: PathBuf) -> Self {
        Self { bus_root }
    }

    fn emit_terminal_batch(&self, jobs: &[TransferJob]) {
        if let [job] = jobs {
            self.emit_terminal(job);
            return;
        }
        let done = jobs
            .iter()
            .filter(|j| j.state == TransferState::Done)
            .count();
        let failed = jobs
            .iter()
            .filter(|j| j.state == TransferState::Failed)
            .count();
        let severity = if failed > 0 { "warning" } else { "info" };
        let summary = match (done, failed) {
            (done, 0) => format!("{done} transfers completed"),
            (0, failed) => format!("{failed} transfers failed"),
            (done, failed) => format!("{done} transfers completed, {failed} failed"),
        };
        self.emit_body(severity, summary, None, None, None, None, None);
    }

    fn emit_terminal(&self, job: &TransferJob) {
        let (severity, summary) = match job.state {
            TransferState::Done => (
                "info",
                format!("transfer {} completed ({})", short_id(&job.id), job.method),
            ),
            TransferState::Failed => (
                "warning",
                format!(
                    "transfer {} failed ({}){}",
                    short_id(&job.id),
                    job.method,
                    job.error
                        .as_deref()
                        .filter(|e| !e.is_empty())
                        .map_or_else(String::new, |e| format!(": {e}"))
                ),
            ),
            _ => return,
        };
        self.emit_body(
            severity,
            summary,
            Some(&job.id),
            Some(job.state.as_str()),
            Some(job.method.as_str()),
            None,
            None,
        );
    }

    fn emit_v2_terminal(&self, job: &TransferJobV2, checksum_sha256: Option<&str>) {
        let (severity, status) = match job.state {
            V2State::Completed => ("info", "completed"),
            V2State::Failed => ("warning", "failed"),
            V2State::Canceled => ("info", "canceled"),
            _ => return,
        };
        let id = job.transfer.to_string();
        self.emit_body(
            severity,
            format!(
                "transfer {} {status} ({})",
                short_id(&id),
                job.kind.as_str()
            ),
            Some(&id),
            Some(status),
            Some(job.kind.as_str()),
            Some(job.progress.bytes_done),
            checksum_sha256,
        );
    }

    fn emit_body(
        &self,
        severity: &str,
        summary: String,
        transfer_id: Option<&str>,
        transfer_state: Option<&str>,
        method: Option<&str>,
        bytes_done: Option<u64>,
        checksum_sha256: Option<&str>,
    ) {
        let body = TransferNotifyBody {
            severity,
            source: "transfers",
            summary,
            host: std::env::var("HOSTNAME").unwrap_or_else(|_| "local".into()),
            ts_unix_ms: now_ms() as i64,
            transfer_id,
            transfer_state,
            method,
            bytes_done,
            checksum_sha256,
        };
        let Ok(json) = serde_json::to_string(&body) else {
            return;
        };
        match Persist::open(self.bus_root.clone()) {
            Ok(persist) => {
                if let Err(e) =
                    persist.write(TRANSFER_NOTIFY_TOPIC, Priority::Default, None, Some(&json))
                {
                    tracing::debug!(target: "mackesd::transfers", error = %e, "transfer notify publish failed");
                }
            }
            Err(e) => {
                tracing::debug!(target: "mackesd::transfers", error = %e, "transfer notify persist open failed");
            }
        }
    }
}

#[derive(Serialize)]
struct TransferNotifyBody<'a> {
    severity: &'a str,
    source: &'a str,
    summary: String,
    host: String,
    ts_unix_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    transfer_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transfer_state: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    method: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes_done: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    checksum_sha256: Option<&'a str>,
}

fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::action_auth::ActionAuthorizer;
    use ed25519_dalek::SigningKey;
    use mackes_mesh_types::cloud::CloudArmSigner;
    use mde_collab_core::{ActorLog, CollabEngine, Ed25519Signer, FileActorLog, RandomIds};
    use mde_collab_types::{
        topics, ActorId, ChecksumPolicy, FileReferenceView, OpaqueNodeRef, SpaceId, SpaceKind,
        TransferControlV2, TransferDirection, TransferEndpoint, TransferKind, TransferOperation,
    };
    use std::path::Path;
    use tokio::sync::watch;
    use uuid::Uuid;

    /// A lane that blocks until a watch gate flips true — lets a test hold jobs in
    /// `Running` to observe the cap, then release them to observe drain. A watch
    /// (not `Notify`) so the release is never lost to a task that hasn't yet parked.
    struct BlockingLane {
        release: watch::Receiver<bool>,
    }

    #[async_trait::async_trait]
    impl LaneRunner for BlockingLane {
        async fn run(&self, _job: &TransferJob, _progress: ProgressSink) -> LaneOutcome {
            let mut rx = self.release.clone();
            let _ = rx.wait_for(|v| *v).await;
            LaneOutcome::Done
        }
    }

    struct ImmediateLane {
        outcome: LaneOutcome,
    }

    struct UnavailableFilesResolver;

    impl FilesEndpointResolver for UnavailableFilesResolver {
        fn resolve(
            &self,
            _identity: &TransferLocation,
            _role: FilesEndpointRole,
        ) -> Result<ResolvedFilesEndpoint, FilesResolveFailure> {
            Err(FilesResolveFailure::Unavailable)
        }
    }

    #[async_trait::async_trait]
    impl LaneRunner for ImmediateLane {
        async fn run(&self, _job: &TransferJob, _progress: ProgressSink) -> LaneOutcome {
            self.outcome.clone()
        }
    }

    fn engine_with(store: &Path, cap: usize, lane: Arc<dyn LaneRunner>) -> Engine {
        Engine {
            queue: TransferQueue::open(store, cap).unwrap(),
            v2_ledger: V2Ledger::open(store).unwrap(),
            sync_pairs: SyncPairStore::open(store).unwrap(),
            tasks: HashMap::new(),
            v2_tasks: HashMap::new(),
            lane,
            files_resolver: Arc::new(UnavailableFilesResolver),
            v2_ledger_lock: Arc::new(Mutex::new(())),
            cap,
            store_root: store.to_path_buf(),
            notify: None,
        }
    }

    fn engine_with_notify(
        store: &Path,
        bus: &Path,
        cap: usize,
        lane: Arc<dyn LaneRunner>,
    ) -> Engine {
        Engine {
            queue: TransferQueue::open(store, cap).unwrap(),
            v2_ledger: V2Ledger::open(store).unwrap(),
            sync_pairs: SyncPairStore::open(store).unwrap(),
            tasks: HashMap::new(),
            v2_tasks: HashMap::new(),
            lane,
            files_resolver: Arc::new(UnavailableFilesResolver),
            v2_ledger_lock: Arc::new(Mutex::new(())),
            cap,
            store_root: store.to_path_buf(),
            notify: Some(TransferNotifier::new(bus.to_path_buf())),
        }
    }

    fn job() -> TransferJob {
        TransferJob::new("/src", "/dst", Method::Rsync, TransferPolicy::default())
    }

    fn v2_job() -> TransferJobV2 {
        TransferJobV2::new(
            TransferId::from_uuid(Uuid::from_u128(0x801)),
            TransferKind::Mesh,
            TransferEndpoint::new(
                TransferLocation::Mesh {
                    node: OpaqueNodeRef::new("peer-oak").unwrap(),
                    object: FileRefId::from_uuid(Uuid::from_u128(0x802)),
                },
                TransferLocation::Local {
                    object: FileRefId::from_uuid(Uuid::from_u128(0x803)),
                },
            ),
            TransferOperation::Copy {
                direction: TransferDirection::Inbound,
            },
            ChecksumPolicy::verify(),
            None,
            100,
        )
        .unwrap()
    }

    fn v2_local_job() -> TransferJobV2 {
        TransferJobV2::new(
            TransferId::from_uuid(Uuid::from_u128(0x811)),
            TransferKind::Local,
            TransferEndpoint::new(
                TransferLocation::Local {
                    object: FileRefId::from_uuid(Uuid::from_u128(0x802)),
                },
                TransferLocation::Local {
                    object: FileRefId::from_uuid(Uuid::from_u128(0x803)),
                },
            ),
            TransferOperation::Copy {
                direction: TransferDirection::Inbound,
            },
            ChecksumPolicy::verify(),
            None,
            100,
        )
        .unwrap()
    }

    fn canonical_file(content_root: &Path, name: &str, bytes: &[u8]) -> (FileRef, PathBuf) {
        let sha256_hex = mde_collab_types::sha256_hex(bytes);
        let path = content_root.join(&sha256_hex[..2]).join(&sha256_hex);
        std::fs::create_dir_all(path.parent().expect("content shard")).unwrap();
        std::fs::write(&path, bytes).unwrap();
        (
            FileRef {
                name: name.to_string(),
                size: bytes.len() as u64,
                sha256_hex,
                mime: Some("application/octet-stream".to_string()),
            },
            path,
        )
    }

    fn publish_file_identities(bus: &Path, source: FileRef, destination: FileRef) -> SpaceId {
        let space = SpaceId::from_uuid(Uuid::from_u128(0x8f0));
        let body = serde_json::to_string(&FileReferences {
            space,
            files: vec![
                FileReferenceView {
                    file: FileRefId::from_uuid(Uuid::from_u128(0x802)),
                    reference: source,
                    linked_by: ActorId::new("peer-oak"),
                    linked_unix_ms: 700,
                },
                FileReferenceView {
                    file: FileRefId::from_uuid(Uuid::from_u128(0x803)),
                    reference: destination,
                    linked_by: ActorId::new("local-seat"),
                    linked_unix_ms: 300,
                },
            ],
        })
        .unwrap();
        Persist::open(bus.to_path_buf())
            .unwrap()
            .write(
                &topics::space_state_topic(topics::projection::FILE_REFERENCES, space),
                Priority::Default,
                None,
                Some(&body),
            )
            .unwrap();
        space
    }

    fn seed_collab_file_log(
        log_root: &Path,
        actor: &str,
        signing_key: SigningKey,
        source: FileRef,
        destination: FileRef,
        created_ms: i64,
    ) -> SpaceId {
        let actor = ActorId::new(actor);
        let signer = Ed25519Signer::new(signing_key);
        let mut ids = RandomIds;
        let mut engine = CollabEngine::in_memory(actor.clone()).unwrap();
        let space = engine
            .apply(
                &CollabCommand::CreateSpace {
                    kind: SpaceKind::Team,
                    name: "transfer-fixture".into(),
                },
                &signer,
                &mut ids,
                created_ms,
            )
            .unwrap()[0]
            .space_id;
        engine
            .apply(
                &CollabCommand::LinkFile {
                    space,
                    file: FileRefId::from_uuid(Uuid::from_u128(0x802)),
                    reference: source,
                },
                &signer,
                &mut ids,
                created_ms + 1,
            )
            .unwrap();
        engine
            .apply(
                &CollabCommand::LinkFile {
                    space,
                    file: FileRefId::from_uuid(Uuid::from_u128(0x803)),
                    reference: destination,
                },
                &signer,
                &mut ids,
                created_ms + 2,
            )
            .unwrap();
        let mut log = FileActorLog::open(log_root, space, &actor).unwrap();
        for event in engine.all_events() {
            log.append(&event).unwrap();
        }
        space
    }

    async fn wait_for_file_projection(
        bus: &Path,
        space: SpaceId,
        file: FileRefId,
        expected_hash: &str,
    ) -> FileReferenceView {
        let topic = topics::space_state_topic(topics::projection::FILE_REFERENCES, space);
        for _ in 0..200 {
            if let Some(row) = Persist::open(bus.to_path_buf())
                .ok()
                .and_then(|persist| persist.read_latest(&topic).ok().flatten())
                .and_then(|message| message.body)
                .and_then(|body| serde_json::from_str::<FileReferences>(&body).ok())
                .and_then(|rows| rows.files.into_iter().find(|row| row.file == file))
                .filter(|row| row.reference.sha256_hex == expected_hash)
            {
                return row;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("canonical Files projection did not reach the expected generation");
    }

    #[test]
    fn store_root_and_cap_resolve_from_env() {
        // Cap parses + floors at 1; a bogus value falls back to the default.
        assert!(default_cap() >= 1);
        assert_eq!(DEFAULT_PARALLEL_CAP, 3);
        // The default store path ends in `transfers`.
        assert!(default_store_root().ends_with("transfers"));
    }

    #[test]
    fn worker_name_is_the_census_token() {
        let w = TransfersWorker::new(PathBuf::from("/tmp/x"));
        assert_eq!(w.name(), "transfers");
    }

    #[tokio::test]
    async fn inbox_submit_is_drained_then_gated_lane_fails_it_honestly() {
        let tmp = tempfile::tempdir().unwrap();
        let mut engine = engine_with(tmp.path(), 2, Arc::new(GatedLaneRunner));
        let j = job();
        let id = j.id.clone();
        write_verb(tmp.path(), &TransferVerb::Submit(j)).unwrap();
        // Drive ticks until the job reaches a terminal state (the gated lane returns
        // immediately, so this settles within a couple of yields — bounded loop).
        for _ in 0..50 {
            engine.tick().await;
            if engine.queue.get(&id).is_some_and(|j| j.state.is_terminal())
                && engine.tasks.is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        let done = engine.queue.get(&id).expect("job in ledger");
        assert_eq!(
            done.state,
            TransferState::Failed,
            "honest gate fails the job"
        );
        assert!(
            done.error.as_deref().unwrap_or_default().contains("rsync"),
            "the failure names the un-wired lane: {:?}",
            done.error
        );
    }

    #[tokio::test]
    async fn v2_collab_files_destination_without_commit_authority_is_safely_gated() {
        let tmp = tempfile::tempdir().unwrap();
        let bus = tempfile::tempdir().unwrap();
        let mesh = tempfile::tempdir().unwrap();
        let content_root = mesh.path().join("collab/content");
        std::fs::create_dir_all(&content_root).unwrap();
        let source_bytes = b"canonical source generation";
        let destination_bytes = b"old destination generation";
        let (source, _source_path) = canonical_file(&content_root, "source.bin", source_bytes);
        let (destination, destination_path) =
            canonical_file(&content_root, "destination.bin", destination_bytes);
        publish_file_identities(bus.path(), source, destination);

        let mut engine = engine_with_notify(tmp.path(), bus.path(), 2, Arc::new(GatedLaneRunner));
        engine.files_resolver = Arc::new(CollabFilesResolver::new(
            Some(bus.path().to_path_buf()),
            content_root,
        ));
        let job = v2_job();
        let id = job.transfer;
        write_verb(tmp.path(), &TransferVerb::SubmitV2(job)).unwrap();
        for _ in 0..100 {
            engine.tick().await;
            if engine
                .v2_ledger
                .get(id)
                .is_some_and(|job| job.state == V2State::Failed)
                && engine.v2_tasks.is_empty()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        let failed = engine.v2_ledger.get(id).expect("durable V2 terminal row");
        assert_eq!(failed.state, V2State::Failed);
        assert_eq!(failed.progress.phase, TransferPhase::Failed);
        assert_eq!(failed.progress.bytes_done, 0);
        assert_eq!(
            failed.progress.error.expect("typed terminal error").code,
            TransferErrorCode::Unsupported
        );
        assert_eq!(
            std::fs::read(destination_path).unwrap(),
            destination_bytes,
            "read-only FileRef authority must never mutate the old hash path"
        );
        assert!(
            engine.queue.list().is_empty(),
            "opaque V2 job never enters legacy path executor"
        );
        let notification = Persist::open(bus.path().to_path_buf())
            .unwrap()
            .read_latest(TRANSFER_NOTIFY_TOPIC)
            .unwrap()
            .expect("terminal notification");
        let body = notification.body.expect("terminal body");
        assert!(body.contains("\"bytes_done\":0"));
        assert!(!body.contains("checksum_sha256"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn v2_production_files_commit_is_corrected_forward_and_retry_idempotent() {
        const AUTH_KEY: &[u8] = b"files-generation-authority-test-key";

        let tmp = tempfile::tempdir().unwrap();
        let bus = tempfile::tempdir().unwrap();
        let workgroup = tempfile::tempdir().unwrap();
        let content_root = workgroup.path().join("collab/content");
        let log_root = workgroup.path().join("collab/logs");
        std::fs::create_dir_all(&content_root).unwrap();
        let source_bytes = b"canonical source generation";
        let destination_bytes = b"old destination generation";
        let (source, source_path) = canonical_file(&content_root, "source.bin", source_bytes);
        let source_hash = source.sha256_hex.clone();
        let (destination, destination_path) =
            canonical_file(&content_root, "destination.bin", destination_bytes);
        let destination_hash = destination.sha256_hex.clone();
        let signing_key = SigningKey::from_bytes(&[23; 32]);
        let auth_now = i64::try_from(now_ms()).unwrap();
        let space = seed_collab_file_log(
            &log_root,
            "eagle",
            signing_key.clone(),
            source,
            destination,
            auth_now - 10_000,
        );

        let mut collab = crate::workers::collab::CollabWorker::new(
            workgroup.path().to_path_buf(),
            "eagle".into(),
            signing_key,
        )
        .with_bus_root(bus.path().to_path_buf())
        .with_log_root(log_root)
        .with_poll_interval(Duration::from_millis(2))
        .with_authorizer(Arc::new(ActionAuthorizer::for_test(
            AUTH_KEY,
            tmp.path().join("collab-auth"),
            auth_now + 5_000,
        )));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let collab_handle =
            tokio::spawn(
                async move { collab.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );
        let initial = wait_for_file_projection(
            bus.path(),
            space,
            FileRefId::from_uuid(Uuid::from_u128(0x803)),
            &destination_hash,
        )
        .await;

        let resolver =
            CollabFilesResolver::new(Some(bus.path().to_path_buf()), content_root.clone())
                .with_commit_authority(
                    CloudArmSigner::new(AUTH_KEY.to_vec()).unwrap(),
                    "eagle",
                    Duration::from_secs(2),
                )
                .fail_next_publications(1);
        let mut engine = engine_with(tmp.path(), 1, Arc::new(GatedLaneRunner));
        engine.files_resolver = Arc::new(resolver);
        let job = v2_local_job();
        let id = job.transfer;
        write_verb(tmp.path(), &TransferVerb::SubmitV2(job)).unwrap();
        for _ in 0..200 {
            engine.tick().await;
            if engine
                .v2_ledger
                .get(id)
                .is_some_and(|job| job.state == V2State::Failed)
                && engine.v2_tasks.is_empty()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let first_failure = engine.v2_ledger.get(id).expect("first terminal attempt");
        let error = first_failure
            .progress
            .error
            .as_ref()
            .expect("typed failure");
        assert_eq!(error.code, TransferErrorCode::Internal);
        assert!(error.retryable);
        assert_eq!(std::fs::read(&destination_path).unwrap(), destination_bytes);
        assert_eq!(std::fs::read(&source_path).unwrap(), source_bytes);
        let still_old = wait_for_file_projection(
            bus.path(),
            space,
            FileRefId::from_uuid(Uuid::from_u128(0x803)),
            &destination_hash,
        )
        .await;
        assert_eq!(still_old.linked_unix_ms, initial.linked_unix_ms);

        engine
            .v2_ledger
            .apply_control(
                id,
                TransferControlV2::Retry,
                first_failure.updated_unix_ms + 1,
            )
            .unwrap();
        for _ in 0..400 {
            engine.tick().await;
            if engine
                .v2_ledger
                .get(id)
                .is_some_and(|job| job.state == V2State::Completed)
                && engine.v2_tasks.is_empty()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let completed = engine.v2_ledger.get(id).expect("retry terminal row");
        assert_eq!(completed.state, V2State::Completed);
        assert_eq!(completed.progress.bytes_done, source_bytes.len() as u64);
        let committed = wait_for_file_projection(
            bus.path(),
            space,
            FileRefId::from_uuid(Uuid::from_u128(0x803)),
            &source_hash,
        )
        .await;
        assert!(committed.linked_unix_ms > initial.linked_unix_ms);
        assert_eq!(committed.reference.name, "destination.bin");
        assert_eq!(std::fs::read(&destination_path).unwrap(), destination_bytes);
        assert_eq!(std::fs::read(&source_path).unwrap(), source_bytes);

        shutdown_tx.send(true).unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(2), collab_handle)
            .await
            .unwrap()
            .unwrap()
            .is_ok());
    }

    #[test]
    fn v2_executor_registry_classifies_every_contract_pair_without_fake_providers() {
        use std::collections::HashSet;

        let expected = [
            (TransferKind::Local, V2OperationKind::Copy),
            (TransferKind::Mesh, V2OperationKind::Copy),
            (TransferKind::Rsync, V2OperationKind::Sync),
            (TransferKind::Sftp, V2OperationKind::Copy),
            (TransferKind::Sftp, V2OperationKind::Download),
            (TransferKind::Sftp, V2OperationKind::Upload),
            (TransferKind::Http, V2OperationKind::Download),
            (TransferKind::Scrape, V2OperationKind::Scrape),
            (TransferKind::Multipart, V2OperationKind::Upload),
            (TransferKind::Recurring, V2OperationKind::Mirror),
            (TransferKind::Clipboard, V2OperationKind::PublishClipboard),
        ];
        let actual = V2_EXECUTOR_REGISTRY
            .iter()
            .map(|row| (row.kind, row.operation))
            .collect::<HashSet<_>>();

        assert_eq!(actual.len(), V2_EXECUTOR_REGISTRY.len(), "duplicate row");
        assert_eq!(actual, expected.into_iter().collect());
        assert_eq!(
            V2_EXECUTOR_REGISTRY
                .iter()
                .filter(|row| row.admission == V2ExecutorAdmission::LocalFilesCopy)
                .count(),
            1,
            "only the production Local Files copy executor is reachable"
        );
        assert!(V2_EXECUTOR_REGISTRY.iter().all(|row| matches!(
            row.admission,
            V2ExecutorAdmission::LocalFilesCopy | V2ExecutorAdmission::Blocked(_)
        )));
        for row in V2_EXECUTOR_REGISTRY {
            if let V2ExecutorAdmission::Blocked(provider) = row.admission {
                assert!(TransferError::new(
                    TransferErrorCode::Unsupported,
                    false,
                    Some(provider.to_string())
                )
                .is_ok());
            }
        }
    }

    #[tokio::test]
    async fn v2_executor_registry_blocks_mesh_without_remote_acknowledgement_provider() {
        struct MustNotResolveMesh;

        impl FilesEndpointResolver for MustNotResolveMesh {
            fn resolve(
                &self,
                _identity: &TransferLocation,
                _role: FilesEndpointRole,
            ) -> Result<ResolvedFilesEndpoint, FilesResolveFailure> {
                panic!("blocked Mesh executor must not resolve a local cache entry")
            }
        }

        let tmp = tempfile::tempdir().unwrap();
        let mut engine = engine_with(tmp.path(), 1, Arc::new(GatedLaneRunner));
        engine.files_resolver = Arc::new(MustNotResolveMesh);
        let job = v2_job();
        let id = job.transfer;
        write_verb(tmp.path(), &TransferVerb::SubmitV2(job)).unwrap();
        for _ in 0..100 {
            engine.tick().await;
            if engine
                .v2_ledger
                .get(id)
                .is_some_and(|job| job.state == V2State::Failed)
                && engine.v2_tasks.is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }

        let failed = engine.v2_ledger.get(id).expect("durable blocked row");
        let error = failed.progress.error.expect("named provider blocker");
        assert_eq!(error.code, TransferErrorCode::Unsupported);
        assert!(!error.retryable);
        assert_eq!(
            error.detail.as_deref(),
            Some("authenticated mesh transport and remote acknowledgement provider is unavailable")
        );
        assert_eq!(failed.progress.bytes_done, 0);
    }

    #[tokio::test]
    async fn v2_unsupported_protocol_fails_with_typed_terminal_error() {
        use mde_collab_types::{OpaqueProfileRef, OpaqueResourceRef};

        let tmp = tempfile::tempdir().unwrap();
        let mut engine = engine_with(tmp.path(), 2, Arc::new(GatedLaneRunner));
        let job = TransferJobV2::new(
            TransferId::from_uuid(Uuid::from_u128(0x8a1)),
            TransferKind::Http,
            TransferEndpoint::new(
                TransferLocation::Http {
                    profile: OpaqueProfileRef::new("public-downloads").unwrap(),
                    resource: OpaqueResourceRef::new("release-object").unwrap(),
                },
                TransferLocation::Local {
                    object: FileRefId::from_uuid(Uuid::from_u128(0x8a2)),
                },
            ),
            TransferOperation::Download,
            ChecksumPolicy::verify(),
            None,
            100,
        )
        .unwrap();
        let id = job.transfer;
        write_verb(tmp.path(), &TransferVerb::SubmitV2(job)).unwrap();
        for _ in 0..100 {
            engine.tick().await;
            if engine
                .v2_ledger
                .get(id)
                .is_some_and(|job| job.state == V2State::Failed)
                && engine.v2_tasks.is_empty()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let failed = engine.v2_ledger.get(id).expect("typed terminal failure");
        assert_eq!(failed.state, V2State::Failed);
        assert_eq!(failed.progress.bytes_done, 0);
        assert_eq!(
            failed.progress.error.expect("typed error").code,
            TransferErrorCode::Unsupported
        );
    }

    #[tokio::test]
    async fn cap_bounds_concurrent_running_jobs() {
        let tmp = tempfile::tempdir().unwrap();
        let (release_tx, release_rx) = watch::channel(false);
        let lane = Arc::new(BlockingLane {
            release: release_rx,
        });
        let mut engine = engine_with(tmp.path(), 2, lane);
        // Three jobs submitted; the blocking lane holds each Running job's slot.
        for _ in 0..3 {
            write_verb(tmp.path(), &TransferVerb::Submit(job())).unwrap();
        }
        engine.tick().await; // drain 3 submits + fill up to the cap
        assert_eq!(engine.queue.running_count(), 2, "cap holds at 2");
        assert_eq!(engine.tasks.len(), 2, "only 2 lane tasks in flight");
        let queued = engine
            .queue
            .list()
            .into_iter()
            .filter(|j| j.state == TransferState::Queued)
            .count();
        assert_eq!(queued, 1, "the third waits Queued behind the cap");
        // Release the lanes: the two finish, freeing slots, and the third — held
        // behind the cap — is then admitted and drains too. All three reach Done.
        release_tx.send(true).unwrap();
        let done_count = |engine: &Engine| {
            engine
                .queue
                .list()
                .into_iter()
                .filter(|j| j.state == TransferState::Done)
                .count()
        };
        for _ in 0..100 {
            engine.tick().await;
            if done_count(&engine) == 3 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            done_count(&engine),
            3,
            "every job — incl. the one held behind the cap — drained"
        );
    }

    #[tokio::test]
    async fn worker_exits_promptly_on_shutdown() {
        let tmp = tempfile::tempdir().unwrap();
        let mut w =
            TransfersWorker::new(tmp.path().to_path_buf()).with_poll(Duration::from_millis(10));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(true).expect("signal shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "worker must exit promptly on shutdown");
        assert!(joined.unwrap().expect("join").is_ok());
    }

    #[tokio::test]
    async fn failed_transfer_emits_one_notify_alert() {
        let tmp = tempfile::tempdir().unwrap();
        let bus = tempfile::tempdir().unwrap();
        let lane = Arc::new(ImmediateLane {
            outcome: LaneOutcome::failed("fixture failure"),
        });
        let mut engine = engine_with_notify(tmp.path(), bus.path(), 1, lane);
        write_verb(tmp.path(), &TransferVerb::Submit(job())).unwrap();
        for _ in 0..20 {
            engine.tick().await;
            if engine.tasks.is_empty()
                && engine
                    .queue
                    .list()
                    .iter()
                    .any(|j| j.state == TransferState::Failed)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        let persist = Persist::open(bus.path().to_path_buf()).unwrap();
        let msgs = persist.list_since(TRANSFER_NOTIFY_TOPIC, None).unwrap();
        assert_eq!(msgs.len(), 1);
        let body: serde_json::Value =
            serde_json::from_str(msgs[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(body["source"], "transfers");
        assert_eq!(body["severity"], "warning");
        assert!(body["summary"]
            .as_str()
            .unwrap()
            .contains("fixture failure"));
        assert_eq!(body["transfer_state"], "failed");
    }

    #[tokio::test]
    async fn same_tick_terminal_batch_emits_one_coalesced_notify_alert() {
        let tmp = tempfile::tempdir().unwrap();
        let bus = tempfile::tempdir().unwrap();
        let lane = Arc::new(ImmediateLane {
            outcome: LaneOutcome::Done,
        });
        let mut engine = engine_with_notify(tmp.path(), bus.path(), 3, lane);
        for _ in 0..3 {
            write_verb(tmp.path(), &TransferVerb::Submit(job())).unwrap();
        }
        for _ in 0..20 {
            engine.tick().await;
            if engine.tasks.is_empty()
                && engine
                    .queue
                    .list()
                    .iter()
                    .filter(|j| j.state == TransferState::Done)
                    .count()
                    == 3
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        let persist = Persist::open(bus.path().to_path_buf()).unwrap();
        let msgs = persist.list_since(TRANSFER_NOTIFY_TOPIC, None).unwrap();
        assert_eq!(msgs.len(), 1, "batch coalesces to one notification");
        let body: serde_json::Value =
            serde_json::from_str(msgs[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(body["severity"], "info");
        assert_eq!(body["summary"], "3 transfers completed");
        assert!(body.get("transfer_id").is_none());
    }

    #[tokio::test]
    async fn due_sync_pair_enqueues_once_then_waits_for_interval() {
        let tmp = tempfile::tempdir().unwrap();
        let mut engine = engine_with(
            tmp.path(),
            1,
            Arc::new(ImmediateLane {
                outcome: LaneOutcome::Done,
            }),
        );
        engine
            .sync_pairs
            .upsert(&SyncPair::new(
                "docs",
                "/src",
                "/dst",
                15,
                TransferPolicy::default(),
            ))
            .unwrap();

        engine.schedule_sync_pairs_at(1_000);
        let first = engine.queue.list();
        assert_eq!(first.len(), 1, "initially due pair fires once");
        assert_eq!(first[0].method, Method::Rsync);
        assert_eq!(first[0].source, "/src");
        assert_eq!(
            engine.sync_pairs.get("docs").unwrap().last_fired_ms,
            Some(1_000)
        );

        engine.schedule_sync_pairs_at(15_999);
        assert_eq!(
            engine.queue.list().len(),
            1,
            "pair does not duplicate before the interval elapses"
        );
        engine.schedule_sync_pairs_at(16_000);
        assert_eq!(
            engine.queue.list().len(),
            2,
            "pair fires again exactly at the next due time"
        );
    }

    #[tokio::test]
    async fn recurring_rsync_pair_fires_and_mirrors_on_tick() {
        if std::process::Command::new("rsync")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("skipping recurring rsync fixture: rsync is not installed");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let source_dir = tmp.path().join("source");
        let dest_dir = tmp.path().join("dest");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(source_dir.join("file.txt"), b"first version").unwrap();

        let mut engine = engine_with(tmp.path().join("store").as_path(), 1, Arc::new(RsyncLane));
        engine
            .sync_pairs
            .upsert(&SyncPair::new(
                "mirror",
                format!("{}/", source_dir.display()),
                format!("{}/", dest_dir.display()),
                5,
                TransferPolicy::default(),
            ))
            .unwrap();

        for _ in 0..1_000 {
            engine.tick().await;
            if engine.tasks.is_empty()
                && engine
                    .queue
                    .list()
                    .iter()
                    .any(|j| j.state == TransferState::Done)
                && std::fs::read(dest_dir.join("file.txt"))
                    .is_ok_and(|bytes| bytes.as_slice() == b"first version")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            std::fs::read(dest_dir.join("file.txt")).unwrap(),
            b"first version"
        );
        assert!(
            engine
                .queue
                .list()
                .iter()
                .any(|j| j.state == TransferState::Done),
            "recurring rsync job should complete successfully: {:?}",
            engine.queue.list()
        );
        assert_eq!(
            engine
                .sync_pairs
                .get("mirror")
                .unwrap()
                .last_fired_ms
                .is_some(),
            true
        );
    }
}
