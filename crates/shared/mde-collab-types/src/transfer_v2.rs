//! Strict V2 contracts for the universal Files transfer lane (WL-FUNC-011).
//!
//! [`TransferJobV2`] is deliberately only a contract.  It separates the
//! source/destination [`TransferEndpoint`] from the [`TransferOperation`]
//! performed on that route, so executors can share one durable ledger without
//! smuggling tool-specific command lines, filesystem paths, URLs, or secrets
//! through the collaboration wire.
//!
//! Endpoint identities are opaque references.  A worker resolves them against
//! node-local registries after admission; the job itself never contains a
//! credential, a path, a command, or a URL.  The custom `Deserialize`
//! implementation admits only the supported schema and validates all bounded
//! fields before a caller can use the job.  Use [`TransferJobV2::from_json_bytes`]
//! at untrusted JSON boundaries so the encoded body is bounded before serde
//! allocates it.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

use crate::ids::{FileRefId, TransferId};
use crate::value::{PayloadRef, TransferDirection, TransferState};

/// The only `TransferJob` V2 schema currently admitted by this crate.
pub const TRANSFER_JOB_V2_SCHEMA_VERSION: u16 = 2;
/// Maximum encoded JSON body accepted by [`TransferJobV2::from_json_bytes`].
pub const MAX_TRANSFER_JOB_V2_JSON_BYTES: usize = 256 * 1024;
/// Maximum size of an opaque node/profile/resource reference.
pub const MAX_TRANSFER_OPAQUE_REF_BYTES: usize = 128;
/// Maximum size of a safe, redacted operator error detail.
pub const MAX_TRANSFER_ERROR_DETAIL_BYTES: usize = 256;
/// Maximum total payload size represented by one transfer.
pub const MAX_TRANSFER_CONTENT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Maximum number of attempts retained by one job.
pub const MAX_TRANSFER_ATTEMPTS: u16 = 32;
/// Maximum reported transfer rate in bytes per second.
pub const MAX_TRANSFER_RATE_BYTES_PER_SECOND: u64 = 1_000_000_000_000;
/// Maximum per-job bandwidth limit in bytes per second.
pub const MAX_TRANSFER_BANDWIDTH_BYTES_PER_SECOND: u64 = 1_000_000_000_000;
/// Maximum recurring interval in seconds.
pub const MAX_TRANSFER_RECURRENCE_SECONDS: u64 = 366 * 24 * 60 * 60;
/// Maximum number of scheduled mirror occurrences.
pub const MAX_TRANSFER_RECURRENCE_RUNS: u32 = 10_000;
/// Maximum MIME/content-type hint accepted on a clipboard payload reference.
pub const MAX_TRANSFER_CONTENT_TYPE_BYTES: usize = 128;

/// The nine executor families in the V2 transfer contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferKind {
    /// A local object-to-object copy on one node.
    Local,
    /// A node-to-node mesh copy using the addressed mesh substrate.
    Mesh,
    /// An rsync-style delta synchronization.
    Rsync,
    /// An SFTP transfer through a sealed profile.
    Sftp,
    /// An HTTP download through an opaque resource/profile registry.
    Http,
    /// A browser-owned scrape session handing materialized output to Files.
    Scrape,
    /// A multipart upload through a sealed profile.
    Multipart,
    /// A recurring mirror using a typed schedule.
    Recurring,
    /// A rich clipboard payload transfer through the Files lane.
    Clipboard,
}

impl TransferKind {
    /// Every V2 kind in stable wire order.
    pub const ALL: [Self; 9] = [
        Self::Local,
        Self::Mesh,
        Self::Rsync,
        Self::Sftp,
        Self::Http,
        Self::Scrape,
        Self::Multipart,
        Self::Recurring,
        Self::Clipboard,
    ];

    /// The canonical snake-case wire token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Mesh => "mesh",
            Self::Rsync => "rsync",
            Self::Sftp => "sftp",
            Self::Http => "http",
            Self::Scrape => "scrape",
            Self::Multipart => "multipart",
            Self::Recurring => "recurring",
            Self::Clipboard => "clipboard",
        }
    }
}

/// An opaque, registry-resolved node identity.
///
/// This is an identity token, not a hostname, address, path, command, or
/// credential.  The node registry resolves it after a job is admitted.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct OpaqueNodeRef(String);

impl OpaqueNodeRef {
    /// Validate and wrap an opaque node token.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` is empty, overlong, or is not a safe
    /// opaque registry token.
    pub fn new(value: impl Into<String>) -> Result<Self, TransferRefValidationError> {
        let value = value.into();
        validate_opaque_ref("node", &value)?;
        Ok(Self(value))
    }

    /// The safe opaque token used for registry lookup.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for OpaqueNodeRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// An opaque reference to a sealed endpoint credential/profile.
///
/// The value is only a lookup key.  Secret material is held by the local
/// profile store and is never represented in a [`TransferJobV2`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct OpaqueProfileRef(String);

impl OpaqueProfileRef {
    /// Validate and wrap an opaque profile token.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` is empty, overlong, unsafe, or resembles
    /// credential material instead of an opaque profile identity.
    pub fn new(value: impl Into<String>) -> Result<Self, TransferRefValidationError> {
        let value = value.into();
        if contains_secret_word(&value) {
            return Err(TransferRefValidationError::ForbiddenValue { field: "profile" });
        }
        validate_opaque_ref("profile", &value)?;
        Ok(Self(value))
    }

    /// The safe opaque lookup token, never the profile's secret value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for OpaqueProfileRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// An opaque resource/object identity resolved by an endpoint registry.
///
/// Resource references intentionally do not carry a path or URL.  They can
/// identify a remote object, HTTP resource, scrape session, or multipart
/// target only through the profile-scoped registry that owns them.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct OpaqueResourceRef(String);

impl OpaqueResourceRef {
    /// Validate and wrap an opaque resource token.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` is empty, overlong, or is not a safe
    /// opaque registry token.
    pub fn new(value: impl Into<String>) -> Result<Self, TransferRefValidationError> {
        let value = value.into();
        validate_opaque_ref("resource", &value)?;
        Ok(Self(value))
    }

    /// The safe opaque resource token.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for OpaqueResourceRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// One side of a transfer route.
///
/// There are no path, URL, shell, environment, username, password, cookie, or
/// token-value fields.  All remote material is selected by opaque registry
/// references; local and mesh objects use the existing opaque [`FileRefId`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum TransferLocation {
    /// A file/object already registered on the local node.
    Local {
        /// Opaque Files object identity.
        object: FileRefId,
    },
    /// A file/object registered on another enrolled mesh node.
    Mesh {
        /// Opaque enrolled-node identity.
        node: OpaqueNodeRef,
        /// Opaque Files object identity on that node.
        object: FileRefId,
    },
    /// An rsync endpoint selected through a sealed profile.
    Rsync {
        /// Opaque profile lookup key.
        profile: OpaqueProfileRef,
        /// Profile-scoped object identity.
        object: OpaqueResourceRef,
    },
    /// An SFTP endpoint selected through a sealed profile.
    Sftp {
        /// Opaque profile lookup key.
        profile: OpaqueProfileRef,
        /// Profile-scoped object identity.
        object: OpaqueResourceRef,
    },
    /// An HTTP resource selected through an opaque profile/resource pair.
    Http {
        /// Opaque profile lookup key; public HTTP also uses a non-secret profile.
        profile: OpaqueProfileRef,
        /// Opaque resource identity, never a URL.
        resource: OpaqueResourceRef,
    },
    /// A browser-owned scrape session and its bounded output identity.
    Scrape {
        /// Opaque browser-session/profile lookup key.
        profile: OpaqueProfileRef,
        /// Opaque scrape output/session identity.
        resource: OpaqueResourceRef,
    },
    /// A multipart target selected through a sealed profile.
    Multipart {
        /// Opaque profile lookup key.
        profile: OpaqueProfileRef,
        /// Opaque upload target identity.
        resource: OpaqueResourceRef,
    },
    /// An out-of-band clipboard payload reference.
    Clipboard {
        /// Opaque clipboard session/profile lookup key.
        profile: OpaqueProfileRef,
        /// Existing content-addressed payload metadata; bytes are not inline.
        payload: PayloadRef,
    },
}

impl TransferLocation {
    /// Return the endpoint family represented by this location.
    #[must_use]
    pub const fn family(&self) -> TransferLocationFamily {
        match self {
            Self::Local { .. } => TransferLocationFamily::Local,
            Self::Mesh { .. } => TransferLocationFamily::Mesh,
            Self::Rsync { .. } => TransferLocationFamily::Rsync,
            Self::Sftp { .. } => TransferLocationFamily::Sftp,
            Self::Http { .. } => TransferLocationFamily::Http,
            Self::Scrape { .. } => TransferLocationFamily::Scrape,
            Self::Multipart { .. } => TransferLocationFamily::Multipart,
            Self::Clipboard { .. } => TransferLocationFamily::Clipboard,
        }
    }
}

/// Closed endpoint families, useful for routing without inspecting opaque data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransferLocationFamily {
    /// Local Files registry.
    Local,
    /// Enrolled mesh node registry.
    Mesh,
    /// Rsync profile registry.
    Rsync,
    /// SFTP profile registry.
    Sftp,
    /// HTTP resource registry.
    Http,
    /// Browser scrape registry.
    Scrape,
    /// Multipart profile registry.
    Multipart,
    /// Clipboard session registry.
    Clipboard,
}

/// The explicit source/destination route for a transfer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferEndpoint {
    /// Where the executor reads the payload.
    pub source: TransferLocation,
    /// Where the executor writes the payload.
    pub destination: TransferLocation,
}

impl TransferEndpoint {
    /// Construct a source/destination route without resolving either endpoint.
    #[must_use]
    pub const fn new(source: TransferLocation, destination: TransferLocation) -> Self {
        Self {
            source,
            destination,
        }
    }
}

/// The typed operation performed on a route.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(
    rename_all = "snake_case",
    tag = "operation",
    content = "options",
    deny_unknown_fields
)]
pub enum TransferOperation {
    /// Copy one registered object to another endpoint.
    Copy {
        /// Direction relative to the local seat, retained for existing ledger views.
        direction: TransferDirection,
    },
    /// Synchronize a source and destination using delta semantics.
    Sync {
        /// Direction relative to the local seat.
        direction: TransferDirection,
        /// Whether the executor may remove destination-only entries.
        delete_extraneous: bool,
    },
    /// Download a selected HTTP/resource object.
    Download,
    /// Upload a selected object to an SFTP or multipart endpoint.
    Upload,
    /// Hand a browser scrape output into the destination.
    Scrape {
        /// Closed output representation selected by the browser adapter.
        output: ScrapeOutputKind,
    },
    /// Run a typed recurring mirror schedule.
    Mirror {
        /// Bounded recurring schedule; no cron or command string is accepted.
        schedule: RecurringSchedule,
    },
    /// Publish a clipboard payload through the Files transfer lane.
    PublishClipboard,
}

/// Closed scrape-output representations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrapeOutputKind {
    /// One materialized file.
    File,
    /// A bounded archive produced by the browser adapter.
    Archive,
    /// A structured export/manifest.
    Manifest,
}

/// A bounded recurring schedule.  A typed interval keeps scheduling data
/// deterministic and prevents cron/executable expressions from entering jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecurringSchedule {
    /// Seconds between mirror occurrences.
    pub every_seconds: u64,
    /// Optional finite occurrence limit; `None` means until disabled.
    #[serde(default)]
    pub max_runs: Option<u32>,
}

impl RecurringSchedule {
    /// Construct a bounded recurring schedule.
    ///
    /// # Errors
    ///
    /// Returns an error when the interval or finite occurrence limit exceeds
    /// the contract bounds.
    pub fn new(
        every_seconds: u64,
        max_runs: Option<u32>,
    ) -> Result<Self, TransferJobV2ValidationError> {
        let schedule = Self {
            every_seconds,
            max_runs,
        };
        schedule.validate()?;
        Ok(schedule)
    }

    /// Validate interval and occurrence bounds.
    ///
    /// # Errors
    ///
    /// Returns an error when the interval is zero/overlong or the finite
    /// occurrence limit is zero/overlong.
    pub fn validate(&self) -> Result<(), TransferJobV2ValidationError> {
        if self.every_seconds == 0 || self.every_seconds > MAX_TRANSFER_RECURRENCE_SECONDS {
            return Err(TransferJobV2ValidationError::OutOfBounds {
                field: "operation.options.schedule.every_seconds",
                max: MAX_TRANSFER_RECURRENCE_SECONDS,
            });
        }
        if self.max_runs == Some(0)
            || self
                .max_runs
                .is_some_and(|runs| runs > MAX_TRANSFER_RECURRENCE_RUNS)
        {
            return Err(TransferJobV2ValidationError::OutOfBounds {
                field: "operation.options.schedule.max_runs",
                max: u64::from(MAX_TRANSFER_RECURRENCE_RUNS),
            });
        }
        Ok(())
    }
}

/// Checksum verification mode for a transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChecksumMode {
    /// Do not verify a checksum on completion.
    Off,
    /// Verify SHA-256; the executor may derive the expected digest from the source.
    Verify,
    /// Require the supplied expected SHA-256 digest.
    Require,
}

/// Bounded checksum policy.  A digest is integrity metadata, not a credential.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChecksumPolicy {
    /// Verification mode.
    pub mode: ChecksumMode,
    /// Optional lower-case expected SHA-256 digest.
    #[serde(default)]
    pub expected_sha256_hex: Option<String>,
}

impl ChecksumPolicy {
    /// The disabled policy.
    #[must_use]
    pub const fn off() -> Self {
        Self {
            mode: ChecksumMode::Off,
            expected_sha256_hex: None,
        }
    }

    /// Verify a derived source/destination SHA-256.
    #[must_use]
    pub const fn verify() -> Self {
        Self {
            mode: ChecksumMode::Verify,
            expected_sha256_hex: None,
        }
    }

    /// Require a particular lower-case SHA-256 digest.
    ///
    /// # Errors
    ///
    /// Returns an error when `expected_sha256_hex` is not exactly a lower-case
    /// SHA-256 digest.
    pub fn require(
        expected_sha256_hex: impl Into<String>,
    ) -> Result<Self, TransferJobV2ValidationError> {
        let policy = Self {
            mode: ChecksumMode::Require,
            expected_sha256_hex: Some(expected_sha256_hex.into()),
        };
        policy.validate()?;
        Ok(policy)
    }

    /// Validate mode/digest consistency and digest syntax.
    ///
    /// # Errors
    ///
    /// Returns an error when the selected mode and optional digest disagree or
    /// the digest is not lower-case hexadecimal SHA-256.
    pub fn validate(&self) -> Result<(), TransferJobV2ValidationError> {
        if let Some(digest) = &self.expected_sha256_hex {
            if !is_lower_sha256(digest) {
                return Err(TransferJobV2ValidationError::InvalidChecksum);
            }
        }
        match (self.mode, self.expected_sha256_hex.is_some()) {
            (ChecksumMode::Off, true) | (ChecksumMode::Require, false) => {
                Err(TransferJobV2ValidationError::InvalidChecksum)
            }
            _ => Ok(()),
        }
    }
}

/// The current phase reported by the real executor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferPhase {
    /// Accepted but not yet claimed by an executor.
    Queued,
    /// Resolving an opaque endpoint reference.
    Resolving,
    /// Establishing the selected protocol session.
    Connecting,
    /// Moving bytes.
    Transferring,
    /// Verifying the configured checksum policy.
    Verifying,
    /// Committing/closing the destination.
    Finalizing,
    /// Held by an operator.
    Paused,
    /// A retry is being scheduled after a real failure.
    Retrying,
    /// Successfully completed.
    Completed,
    /// Cancelled by an operator.
    Canceled,
    /// Failed with the typed error below.
    Failed,
}

/// Closed failure categories.  Details are deliberately redacted/bounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferErrorCode {
    /// A referenced registry object does not exist.
    ReferenceUnavailable,
    /// A sealed profile is unavailable or could not be opened.
    ProfileUnavailable,
    /// The remote endpoint refused the operation.
    RemoteRejected,
    /// The protocol/session could not be established.
    Connection,
    /// A bounded operation timed out.
    Timeout,
    /// The executor lacked permission, without exposing credentials.
    PermissionDenied,
    /// Checksum or size verification failed.
    ChecksumMismatch,
    /// The remote protocol returned an invalid response.
    Protocol,
    /// The requested operation is not supported by the endpoint.
    Unsupported,
    /// The submitted operation was invalid.
    InvalidRequest,
    /// The operator cancelled the job.
    Canceled,
    /// A non-specific executor failure; detail remains redacted.
    Internal,
}

/// A bounded, redacted transfer error.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferError {
    /// Stable machine-readable category.
    pub code: TransferErrorCode,
    /// Whether retrying may succeed without changing the request.
    pub retryable: bool,
    /// Optional operator-safe detail.  It is not a command, path, URL, or secret.
    #[serde(default)]
    pub detail: Option<String>,
}

impl TransferError {
    /// Construct and validate a typed error.
    ///
    /// # Errors
    ///
    /// Returns an error when `detail` is empty, overlong, or contains a path,
    /// command-like token, URL, or credential-like value.
    pub fn new(
        code: TransferErrorCode,
        retryable: bool,
        detail: Option<String>,
    ) -> Result<Self, TransferJobV2ValidationError> {
        let error = Self {
            code,
            retryable,
            detail,
        };
        error.validate()?;
        Ok(error)
    }

    /// Validate the redacted detail bound and forbidden-value rules.
    ///
    /// # Errors
    ///
    /// Returns an error when the optional detail is empty, overlong, or would
    /// disclose a path, command, URL, or credential-like value.
    pub fn validate(&self) -> Result<(), TransferJobV2ValidationError> {
        if let Some(detail) = &self.detail {
            if detail.is_empty() || detail.len() > MAX_TRANSFER_ERROR_DETAIL_BYTES {
                return Err(TransferJobV2ValidationError::OutOfBounds {
                    field: "progress.error.detail",
                    max: u64::try_from(MAX_TRANSFER_ERROR_DETAIL_BYTES).unwrap_or(u64::MAX),
                });
            }
            if contains_forbidden_error_value(detail) {
                return Err(TransferJobV2ValidationError::ForbiddenValue {
                    field: "progress.error.detail",
                });
            }
        }
        Ok(())
    }
}

/// Real progress emitted by a transfer executor; percentages are intentionally
/// not stored because bytes and the optional total are the source of truth.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferProgress {
    /// Bytes actually committed/read so far.
    pub bytes_done: u64,
    /// Total bytes when the selected protocol knows it; `None` is honest unknown.
    #[serde(default)]
    pub total_bytes: Option<u64>,
    /// Last measured real rate, if the executor reported one.
    #[serde(default)]
    pub bytes_per_second: Option<u64>,
    /// Current executor phase.
    pub phase: TransferPhase,
    /// One-based attempt number once an executor has started; zero while queued.
    pub attempt: u16,
    /// Last/terminal typed error, when the phase warrants one.
    #[serde(default)]
    pub error: Option<TransferError>,
}

impl TransferProgress {
    /// The honest pre-execution progress value.
    #[must_use]
    pub const fn queued() -> Self {
        Self {
            bytes_done: 0,
            total_bytes: None,
            bytes_per_second: None,
            phase: TransferPhase::Queued,
            attempt: 0,
            error: None,
        }
    }

    /// Validate byte, rate, attempt, phase, and error invariants.
    ///
    /// # Errors
    ///
    /// Returns an error when byte/rate/attempt bounds, phase/error pairing, or
    /// completion invariants are inconsistent.
    pub fn validate(&self) -> Result<(), TransferJobV2ValidationError> {
        if self.bytes_done > MAX_TRANSFER_CONTENT_BYTES {
            return Err(TransferJobV2ValidationError::OutOfBounds {
                field: "progress.bytes_done",
                max: MAX_TRANSFER_CONTENT_BYTES,
            });
        }
        if let Some(total) = self.total_bytes {
            if total > MAX_TRANSFER_CONTENT_BYTES || self.bytes_done > total {
                return Err(TransferJobV2ValidationError::InvalidProgress {
                    field: "progress.total_bytes",
                });
            }
        }
        if let Some(rate) = self.bytes_per_second {
            if rate == 0 || rate > MAX_TRANSFER_RATE_BYTES_PER_SECOND {
                return Err(TransferJobV2ValidationError::OutOfBounds {
                    field: "progress.bytes_per_second",
                    max: MAX_TRANSFER_RATE_BYTES_PER_SECOND,
                });
            }
        }
        if self.attempt > MAX_TRANSFER_ATTEMPTS {
            return Err(TransferJobV2ValidationError::OutOfBounds {
                field: "progress.attempt",
                max: u64::from(MAX_TRANSFER_ATTEMPTS),
            });
        }
        if let Some(error) = &self.error {
            error.validate()?;
        }
        let phase_requires_attempt = matches!(
            self.phase,
            TransferPhase::Resolving
                | TransferPhase::Connecting
                | TransferPhase::Transferring
                | TransferPhase::Verifying
                | TransferPhase::Finalizing
                | TransferPhase::Retrying
                | TransferPhase::Completed
                | TransferPhase::Failed
        );
        if phase_requires_attempt && self.attempt == 0 {
            return Err(TransferJobV2ValidationError::InvalidProgress {
                field: "progress.attempt",
            });
        }
        if matches!(
            self.phase,
            TransferPhase::Queued | TransferPhase::Paused | TransferPhase::Canceled
        ) && self.error.is_some()
        {
            return Err(TransferJobV2ValidationError::InvalidProgress {
                field: "progress.error",
            });
        }
        if matches!(self.phase, TransferPhase::Failed) && self.error.is_none() {
            return Err(TransferJobV2ValidationError::InvalidProgress {
                field: "progress.error",
            });
        }
        if self.error.is_some()
            && !matches!(self.phase, TransferPhase::Retrying | TransferPhase::Failed)
        {
            return Err(TransferJobV2ValidationError::InvalidProgress {
                field: "progress.error",
            });
        }
        if matches!(self.phase, TransferPhase::Retrying) && self.error.is_none() {
            return Err(TransferJobV2ValidationError::InvalidProgress {
                field: "progress.error",
            });
        }
        if matches!(self.phase, TransferPhase::Completed)
            && self
                .total_bytes
                .is_some_and(|total| self.bytes_done != total)
        {
            return Err(TransferJobV2ValidationError::InvalidProgress {
                field: "progress.bytes_done",
            });
        }
        Ok(())
    }
}

/// A durable V2 transfer job.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransferJobV2 {
    /// Schema discriminator; must equal [`TRANSFER_JOB_V2_SCHEMA_VERSION`].
    pub schema_version: u16,
    /// Existing opaque transfer control identity.
    pub transfer: TransferId,
    /// Executor family.
    pub kind: TransferKind,
    /// Source/destination route, separate from the operation.
    pub endpoint: TransferEndpoint,
    /// Typed operation performed on [`Self::endpoint`].
    pub operation: TransferOperation,
    /// Current shared lifecycle state reused from the collaboration contract.
    pub state: TransferState,
    /// Real executor progress and typed error state.
    pub progress: TransferProgress,
    /// Completion-integrity policy.
    pub checksum: ChecksumPolicy,
    /// Optional per-job bandwidth ceiling.
    #[serde(default)]
    pub bandwidth_limit_bytes_per_second: Option<u64>,
    /// Caller-injected submission timestamp.
    pub created_unix_ms: u64,
    /// Caller-injected last-update timestamp.
    pub updated_unix_ms: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TransferJobV2Wire {
    schema_version: u16,
    transfer: TransferId,
    kind: TransferKind,
    endpoint: TransferEndpoint,
    operation: TransferOperation,
    state: TransferState,
    progress: TransferProgress,
    checksum: ChecksumPolicy,
    #[serde(default)]
    bandwidth_limit_bytes_per_second: Option<u64>,
    created_unix_ms: u64,
    updated_unix_ms: u64,
}

impl<'de> Deserialize<'de> for TransferJobV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = TransferJobV2Wire::deserialize(deserializer)?;
        Self::from_wire(wire).map_err(serde::de::Error::custom)
    }
}

impl TransferJobV2 {
    /// Construct a new queued job with caller-supplied time metadata.
    ///
    /// # Errors
    ///
    /// Returns an error when any endpoint, operation, policy, identifier, or
    /// bound is invalid.
    pub fn new(
        transfer: TransferId,
        kind: TransferKind,
        endpoint: TransferEndpoint,
        operation: TransferOperation,
        checksum: ChecksumPolicy,
        bandwidth_limit_bytes_per_second: Option<u64>,
        created_unix_ms: u64,
    ) -> Result<Self, TransferJobV2ValidationError> {
        Self {
            schema_version: TRANSFER_JOB_V2_SCHEMA_VERSION,
            transfer,
            kind,
            endpoint,
            operation,
            state: TransferState::Queued,
            progress: TransferProgress::queued(),
            checksum,
            bandwidth_limit_bytes_per_second,
            created_unix_ms,
            updated_unix_ms: created_unix_ms,
        }
        .admitted()
    }

    /// Validate the complete intrinsic contract without reading a clock or
    /// resolving any local profile/endpoint registry.
    ///
    /// # Errors
    ///
    /// Returns an error when the schema, route, operation, lifecycle/progress,
    /// checksum policy, timestamps, or any bounded field is invalid.
    pub fn validate(&self) -> Result<(), TransferJobV2ValidationError> {
        if self.schema_version != TRANSFER_JOB_V2_SCHEMA_VERSION {
            return Err(TransferJobV2ValidationError::UnsupportedSchema {
                found: self.schema_version,
            });
        }
        if self.transfer.is_nil() {
            return Err(TransferJobV2ValidationError::NilTransferId);
        }
        if self.updated_unix_ms < self.created_unix_ms {
            return Err(TransferJobV2ValidationError::InvalidTimestamp);
        }
        if let Some(limit) = self.bandwidth_limit_bytes_per_second {
            if limit == 0 || limit > MAX_TRANSFER_BANDWIDTH_BYTES_PER_SECOND {
                return Err(TransferJobV2ValidationError::OutOfBounds {
                    field: "bandwidth_limit_bytes_per_second",
                    max: MAX_TRANSFER_BANDWIDTH_BYTES_PER_SECOND,
                });
            }
        }
        validate_endpoint(&self.endpoint)?;
        validate_operation(&self.operation)?;
        self.checksum.validate()?;
        self.progress.validate()?;
        validate_kind_route_operation(self.kind, &self.endpoint, &self.operation)?;
        validate_state_progress(self.state, &self.progress)?;
        Ok(())
    }

    /// Admit an already-decoded job, returning it only when valid.
    ///
    /// # Errors
    ///
    /// Returns the first intrinsic validation error found in the job.
    pub fn admitted(self) -> Result<Self, TransferJobV2ValidationError> {
        self.validate()?;
        Ok(self)
    }

    /// Decode and admit a bounded JSON body.
    ///
    /// # Errors
    ///
    /// Returns an error for an oversized/malformed body, unknown wire fields,
    /// an unsupported schema, or failed semantic admission.
    pub fn from_json(body: &str) -> Result<Self, TransferJobV2DecodeError> {
        Self::from_json_bytes(body.as_bytes())
    }

    /// Decode and admit a bounded JSON byte body.
    ///
    /// # Errors
    ///
    /// Returns an error for an oversized/malformed body, unknown wire fields,
    /// an unsupported schema, or failed semantic admission.
    pub fn from_json_bytes(body: &[u8]) -> Result<Self, TransferJobV2DecodeError> {
        if body.len() > MAX_TRANSFER_JOB_V2_JSON_BYTES {
            return Err(TransferJobV2DecodeError::BodyTooLarge {
                bytes: body.len(),
                max: MAX_TRANSFER_JOB_V2_JSON_BYTES,
            });
        }
        let wire = serde_json::from_slice::<TransferJobV2Wire>(body)
            .map_err(TransferJobV2DecodeError::Json)?;
        Self::from_wire(wire).map_err(TransferJobV2DecodeError::Validation)
    }

    /// Whether an operator control is legal for the current state.
    #[must_use]
    pub const fn can_control(&self, control: TransferControlV2) -> bool {
        control.is_allowed(self.state)
    }

    fn from_wire(wire: TransferJobV2Wire) -> Result<Self, TransferJobV2ValidationError> {
        Self {
            schema_version: wire.schema_version,
            transfer: wire.transfer,
            kind: wire.kind,
            endpoint: wire.endpoint,
            operation: wire.operation,
            state: wire.state,
            progress: wire.progress,
            checksum: wire.checksum,
            bandwidth_limit_bytes_per_second: wire.bandwidth_limit_bytes_per_second,
            created_unix_ms: wire.created_unix_ms,
            updated_unix_ms: wire.updated_unix_ms,
        }
        .admitted()
    }
}

/// Pause/resume/retry/cancel controls for a V2 job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferControlV2 {
    /// Hold a queued or active job.
    Pause,
    /// Re-arm a paused job.
    Resume,
    /// Re-queue a failed/cancelled job for another attempt.
    Retry,
    /// Preserve cancellation in history and stop the job.
    Cancel,
}

/// Short alias for callers that refer to controls as actions.
pub type TransferAction = TransferControlV2;

impl TransferControlV2 {
    /// Every control in stable wire order.
    pub const ALL: [Self; 4] = [Self::Pause, Self::Resume, Self::Retry, Self::Cancel];

    /// Whether this control is legal for the shared lifecycle state.
    #[must_use]
    pub const fn is_allowed(self, state: TransferState) -> bool {
        match self {
            Self::Pause => matches!(state, TransferState::Queued | TransferState::Active),
            Self::Resume => matches!(state, TransferState::Paused),
            Self::Retry => matches!(state, TransferState::Failed | TransferState::Canceled),
            Self::Cancel => matches!(
                state,
                TransferState::Queued | TransferState::Active | TransferState::Paused
            ),
        }
    }
}

impl From<crate::command::TransferControl> for TransferControlV2 {
    fn from(control: crate::command::TransferControl) -> Self {
        match control {
            crate::command::TransferControl::Pause => Self::Pause,
            crate::command::TransferControl::Resume => Self::Resume,
            crate::command::TransferControl::Cancel => Self::Cancel,
        }
    }
}

/// Why a reference failed admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferRefValidationError {
    /// The token was empty, too long, or not a safe opaque identifier.
    InvalidReference {
        /// Reference family.
        field: &'static str,
    },
    /// A profile-like value contained a credential keyword.
    ForbiddenValue {
        /// Field that contained the forbidden value.
        field: &'static str,
    },
}

impl fmt::Display for TransferRefValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidReference { field } => {
                write!(formatter, "invalid opaque transfer {field} reference")
            }
            Self::ForbiddenValue { field } => {
                write!(formatter, "forbidden transfer value in {field}")
            }
        }
    }
}

impl std::error::Error for TransferRefValidationError {}

/// Why a V2 job was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferJobV2ValidationError {
    /// The schema discriminator is not supported.
    UnsupportedSchema {
        /// Version found on the wire.
        found: u16,
    },
    /// The transfer id was the nil sentinel.
    NilTransferId,
    /// Timestamp ordering is invalid.
    InvalidTimestamp,
    /// A bounded value exceeded its maximum.
    OutOfBounds {
        /// Field that exceeded its bound.
        field: &'static str,
        /// Maximum admitted value.
        max: u64,
    },
    /// Byte progress is internally inconsistent.
    InvalidProgress {
        /// Progress field that failed.
        field: &'static str,
    },
    /// Endpoint shape or payload metadata is invalid.
    InvalidEndpoint {
        /// Endpoint field that failed.
        field: &'static str,
    },
    /// Operation options are invalid.
    InvalidOperation {
        /// Operation field that failed.
        field: &'static str,
    },
    /// The kind, endpoint families, and operation do not agree.
    KindOperationMismatch {
        /// Kind selected by the caller.
        kind: TransferKind,
    },
    /// Checksum mode/digest pairing is invalid.
    InvalidChecksum,
    /// A free-text field attempted to carry a command, path, URL, or secret.
    ForbiddenValue {
        /// Field that contained the forbidden value.
        field: &'static str,
    },
}

impl fmt::Display for TransferJobV2ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchema { found } => {
                write!(
                    formatter,
                    "unsupported transfer job V2 schema version {found}"
                )
            }
            Self::NilTransferId => formatter.write_str("transfer job id is nil"),
            Self::InvalidTimestamp => formatter.write_str("transfer job timestamps are invalid"),
            Self::OutOfBounds { field, max } => {
                write!(formatter, "transfer {field} exceeds bound {max}")
            }
            Self::InvalidProgress { field } => {
                write!(formatter, "invalid transfer progress field {field}")
            }
            Self::InvalidEndpoint { field } => {
                write!(formatter, "invalid transfer endpoint field {field}")
            }
            Self::InvalidOperation { field } => {
                write!(formatter, "invalid transfer operation field {field}")
            }
            Self::KindOperationMismatch { kind } => {
                write!(
                    formatter,
                    "transfer kind {} does not match route/operation",
                    kind.as_str()
                )
            }
            Self::InvalidChecksum => formatter.write_str("invalid transfer checksum policy"),
            Self::ForbiddenValue { field } => {
                write!(formatter, "forbidden transfer value in {field}")
            }
        }
    }
}

impl std::error::Error for TransferJobV2ValidationError {}

/// Why a JSON V2 body could not be decoded and admitted.
#[derive(Debug)]
pub enum TransferJobV2DecodeError {
    /// The encoded body was rejected before serde allocation.
    BodyTooLarge {
        /// Number of bytes supplied.
        bytes: usize,
        /// Maximum encoded body size.
        max: usize,
    },
    /// The body was malformed JSON or had an unknown wire field.
    Json(serde_json::Error),
    /// The body decoded but failed semantic validation.
    Validation(TransferJobV2ValidationError),
}

impl fmt::Display for TransferJobV2DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BodyTooLarge { bytes, max } => {
                write!(
                    formatter,
                    "transfer job body is {bytes} bytes; maximum is {max}"
                )
            }
            Self::Json(error) => write!(formatter, "invalid transfer job JSON: {error}"),
            Self::Validation(error) => write!(formatter, "invalid transfer job: {error}"),
        }
    }
}

impl std::error::Error for TransferJobV2DecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::BodyTooLarge { .. } | Self::Validation(_) => None,
        }
    }
}

fn validate_opaque_ref(field: &'static str, value: &str) -> Result<(), TransferRefValidationError> {
    if value.is_empty()
        || value.len() > MAX_TRANSFER_OPAQUE_REF_BYTES
        || !value.bytes().enumerate().all(|(index, byte)| {
            (byte.is_ascii_alphanumeric() && (index > 0 || byte.is_ascii_alphanumeric()))
                || (index > 0 && matches!(byte, b'-' | b'_' | b'.'))
        })
        || value == "."
        || value == ".."
    {
        return Err(TransferRefValidationError::InvalidReference { field });
    }
    Ok(())
}

fn validate_endpoint(endpoint: &TransferEndpoint) -> Result<(), TransferJobV2ValidationError> {
    validate_location(&endpoint.source, "endpoint.source")?;
    validate_location(&endpoint.destination, "endpoint.destination")?;
    Ok(())
}

fn validate_location(
    location: &TransferLocation,
    field: &'static str,
) -> Result<(), TransferJobV2ValidationError> {
    match location {
        TransferLocation::Local { object } | TransferLocation::Mesh { object, .. } => {
            if object.is_nil() {
                return Err(TransferJobV2ValidationError::InvalidEndpoint { field });
            }
        }
        TransferLocation::Rsync { .. }
        | TransferLocation::Sftp { .. }
        | TransferLocation::Http { .. }
        | TransferLocation::Scrape { .. }
        | TransferLocation::Multipart { .. } => {}
        TransferLocation::Clipboard { payload, .. } => validate_payload_ref(payload, field)?,
    }
    Ok(())
}

fn validate_payload_ref(
    payload: &PayloadRef,
    field: &'static str,
) -> Result<(), TransferJobV2ValidationError> {
    if payload.len > MAX_TRANSFER_CONTENT_BYTES || !is_lower_sha256(&payload.sha256_hex) {
        return Err(TransferJobV2ValidationError::InvalidEndpoint { field });
    }
    if let Some(content_type) = &payload.content_type {
        if content_type.is_empty()
            || content_type.len() > MAX_TRANSFER_CONTENT_TYPE_BYTES
            || content_type
                .bytes()
                .any(|byte| byte.is_ascii_control() || byte == b'\\')
            || content_type.contains("://")
        {
            return Err(TransferJobV2ValidationError::ForbiddenValue { field });
        }
    }
    Ok(())
}

fn validate_operation(operation: &TransferOperation) -> Result<(), TransferJobV2ValidationError> {
    if let TransferOperation::Mirror { schedule } = operation {
        schedule.validate()?;
    }
    Ok(())
}

fn validate_kind_route_operation(
    kind: TransferKind,
    endpoint: &TransferEndpoint,
    operation: &TransferOperation,
) -> Result<(), TransferJobV2ValidationError> {
    let source = endpoint.source.family();
    let destination = endpoint.destination.family();
    let valid = match kind {
        TransferKind::Local => {
            matches!(operation, TransferOperation::Copy { .. })
                && source == TransferLocationFamily::Local
                && destination == TransferLocationFamily::Local
        }
        TransferKind::Mesh => {
            matches!(operation, TransferOperation::Copy { .. })
                && (source == TransferLocationFamily::Mesh
                    || destination == TransferLocationFamily::Mesh)
        }
        TransferKind::Rsync => {
            matches!(operation, TransferOperation::Sync { .. })
                && !matches!(
                    source,
                    TransferLocationFamily::Http | TransferLocationFamily::Scrape
                )
                && !matches!(
                    destination,
                    TransferLocationFamily::Http | TransferLocationFamily::Scrape
                )
        }
        TransferKind::Sftp => {
            matches!(
                operation,
                TransferOperation::Copy { .. }
                    | TransferOperation::Download
                    | TransferOperation::Upload
            ) && (source == TransferLocationFamily::Sftp
                || destination == TransferLocationFamily::Sftp)
        }
        TransferKind::Http => {
            matches!(operation, TransferOperation::Download)
                && source == TransferLocationFamily::Http
        }
        TransferKind::Scrape => {
            matches!(operation, TransferOperation::Scrape { .. })
                && source == TransferLocationFamily::Scrape
        }
        TransferKind::Multipart => {
            matches!(operation, TransferOperation::Upload)
                && destination == TransferLocationFamily::Multipart
        }
        TransferKind::Recurring => {
            matches!(operation, TransferOperation::Mirror { .. })
                && !matches!(
                    source,
                    TransferLocationFamily::Http
                        | TransferLocationFamily::Scrape
                        | TransferLocationFamily::Clipboard
                        | TransferLocationFamily::Multipart
                )
                && !matches!(
                    destination,
                    TransferLocationFamily::Http
                        | TransferLocationFamily::Scrape
                        | TransferLocationFamily::Clipboard
                        | TransferLocationFamily::Multipart
                )
        }
        TransferKind::Clipboard => {
            matches!(operation, TransferOperation::PublishClipboard)
                && source == TransferLocationFamily::Clipboard
        }
    };
    if valid {
        Ok(())
    } else {
        Err(TransferJobV2ValidationError::KindOperationMismatch { kind })
    }
}

fn validate_state_progress(
    state: TransferState,
    progress: &TransferProgress,
) -> Result<(), TransferJobV2ValidationError> {
    let valid = match state {
        TransferState::Queued => progress.phase == TransferPhase::Queued,
        TransferState::Active => matches!(
            progress.phase,
            TransferPhase::Resolving
                | TransferPhase::Connecting
                | TransferPhase::Transferring
                | TransferPhase::Verifying
                | TransferPhase::Finalizing
                | TransferPhase::Retrying
        ),
        TransferState::Paused => progress.phase == TransferPhase::Paused,
        TransferState::Completed => progress.phase == TransferPhase::Completed,
        TransferState::Failed => progress.phase == TransferPhase::Failed,
        TransferState::Canceled => progress.phase == TransferPhase::Canceled,
    };
    if valid {
        Ok(())
    } else {
        Err(TransferJobV2ValidationError::InvalidProgress {
            field: "state/progress.phase",
        })
    }
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn contains_secret_word(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "password",
        "passwd",
        "secret",
        "credential",
        "authorization",
        "cookie",
        "token",
        "private_key",
        "apikey",
    ]
    .iter()
    .any(|word| lower.contains(word))
}

fn contains_forbidden_error_value(value: &str) -> bool {
    if value.chars().any(char::is_control)
        || value.chars().any(|character| {
            matches!(
                character,
                '/' | '\\' | '$' | '`' | ';' | '|' | '&' | '<' | '>'
            )
        })
        || value.contains("://")
        || contains_secret_word(value)
    {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::sha256_hex;
    use serde_json::json;

    fn profile(name: &str) -> OpaqueProfileRef {
        OpaqueProfileRef::new(name).expect("safe profile reference")
    }

    fn resource(name: &str) -> OpaqueResourceRef {
        OpaqueResourceRef::new(name).expect("safe resource reference")
    }

    fn node(name: &str) -> OpaqueNodeRef {
        OpaqueNodeRef::new(name).expect("safe node reference")
    }

    fn local() -> TransferLocation {
        TransferLocation::Local {
            object: FileRefId::new(),
        }
    }

    fn sample_job() -> TransferJobV2 {
        let payload = PayloadRef::of_bytes(b"clipboard payload");
        let endpoint = TransferEndpoint::new(
            TransferLocation::Http {
                profile: profile("http-public"),
                resource: resource("resource-1"),
            },
            local(),
        );
        let job = TransferJobV2 {
            schema_version: TRANSFER_JOB_V2_SCHEMA_VERSION,
            transfer: TransferId::new(),
            kind: TransferKind::Http,
            endpoint,
            operation: TransferOperation::Download,
            state: TransferState::Active,
            progress: TransferProgress {
                bytes_done: 4096,
                total_bytes: Some(8192),
                bytes_per_second: Some(2048),
                phase: TransferPhase::Transferring,
                attempt: 1,
                error: None,
            },
            checksum: ChecksumPolicy::verify(),
            bandwidth_limit_bytes_per_second: Some(10_000),
            created_unix_ms: 100,
            updated_unix_ms: 200,
        };
        assert_eq!(payload.len, 17);
        job.admitted().expect("sample is valid")
    }

    #[test]
    fn v2_job_round_trips_with_separate_endpoint_operation_and_real_progress() {
        let job = sample_job();
        let json = serde_json::to_string(&job).expect("encode");
        let decoded = TransferJobV2::from_json(&json).expect("strict decode");
        assert_eq!(decoded, job);
        assert!(json.contains("\"endpoint\""));
        assert!(json.contains("\"operation\":\"download\""));
        assert!(json.contains("\"bytes_done\":4096"));
        assert!(json.contains("\"phase\":\"transferring\""));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn all_executor_kinds_have_closed_typed_routes() {
        let local_source = local();
        let local_destination = local();
        let cases = [
            (
                TransferKind::Local,
                TransferEndpoint::new(local_source.clone(), local_destination.clone()),
                TransferOperation::Copy {
                    direction: TransferDirection::Inbound,
                },
            ),
            (
                TransferKind::Mesh,
                TransferEndpoint::new(
                    TransferLocation::Mesh {
                        node: node("node-1"),
                        object: FileRefId::new(),
                    },
                    local_destination.clone(),
                ),
                TransferOperation::Copy {
                    direction: TransferDirection::Inbound,
                },
            ),
            (
                TransferKind::Rsync,
                TransferEndpoint::new(
                    TransferLocation::Rsync {
                        profile: profile("rsync-prod"),
                        object: resource("source-1"),
                    },
                    local_destination.clone(),
                ),
                TransferOperation::Sync {
                    direction: TransferDirection::Inbound,
                    delete_extraneous: false,
                },
            ),
            (
                TransferKind::Sftp,
                TransferEndpoint::new(
                    local_source.clone(),
                    TransferLocation::Sftp {
                        profile: profile("sftp-prod"),
                        object: resource("target-1"),
                    },
                ),
                TransferOperation::Upload,
            ),
            (
                TransferKind::Http,
                TransferEndpoint::new(
                    TransferLocation::Http {
                        profile: profile("http-public"),
                        resource: resource("download-1"),
                    },
                    local_destination.clone(),
                ),
                TransferOperation::Download,
            ),
            (
                TransferKind::Scrape,
                TransferEndpoint::new(
                    TransferLocation::Scrape {
                        profile: profile("browser-session"),
                        resource: resource("scrape-output-1"),
                    },
                    local_destination.clone(),
                ),
                TransferOperation::Scrape {
                    output: ScrapeOutputKind::Manifest,
                },
            ),
            (
                TransferKind::Multipart,
                TransferEndpoint::new(
                    local_source.clone(),
                    TransferLocation::Multipart {
                        profile: profile("upload-prod"),
                        resource: resource("multipart-target-1"),
                    },
                ),
                TransferOperation::Upload,
            ),
            (
                TransferKind::Recurring,
                TransferEndpoint::new(
                    local_source,
                    TransferLocation::Mesh {
                        node: node("node-2"),
                        object: FileRefId::new(),
                    },
                ),
                TransferOperation::Mirror {
                    schedule: RecurringSchedule::new(3600, None).expect("schedule"),
                },
            ),
            (
                TransferKind::Clipboard,
                TransferEndpoint::new(
                    TransferLocation::Clipboard {
                        profile: profile("clipboard-session"),
                        payload: PayloadRef::of_bytes(b"clipboard payload"),
                    },
                    local_destination,
                ),
                TransferOperation::PublishClipboard,
            ),
        ];
        assert_eq!(cases.len(), TransferKind::ALL.len());
        for (kind, endpoint, operation) in cases {
            let job = TransferJobV2::new(
                TransferId::new(),
                kind,
                endpoint,
                operation,
                ChecksumPolicy::off(),
                None,
                1,
            )
            .expect("typed kind route admitted");
            assert!(job.validate().is_ok());
        }
    }

    #[test]
    fn unknown_schema_and_unknown_top_level_fields_are_rejected() {
        let mut value = serde_json::to_value(sample_job()).expect("value");
        value["schema_version"] = json!(1);
        assert!(matches!(
            TransferJobV2::from_json(&value.to_string()),
            Err(TransferJobV2DecodeError::Validation(
                TransferJobV2ValidationError::UnsupportedSchema { found: 1 }
            ))
        ));

        let mut hostile = serde_json::to_value(sample_job()).expect("value");
        hostile["command"] = json!("rm -rf /");
        assert!(TransferJobV2::from_json(&hostile.to_string()).is_err());
        let mut path = serde_json::to_value(sample_job()).expect("value");
        path["path"] = json!("/etc/passwd");
        assert!(TransferJobV2::from_json(&path.to_string()).is_err());
        let mut secret = serde_json::to_value(sample_job()).expect("value");
        secret["password"] = json!("hunter2");
        assert!(TransferJobV2::from_json(&secret.to_string()).is_err());

        let mut nested_endpoint = serde_json::to_value(sample_job()).expect("value");
        nested_endpoint["endpoint"]["source"]["command"] = json!("curl");
        assert!(TransferJobV2::from_json(&nested_endpoint.to_string()).is_err());

        let mut nested_operation = serde_json::to_value(sample_job()).expect("value");
        nested_operation["operation"] = json!({
            "operation": "download",
            "options": {"path": "/tmp/escape"}
        });
        assert!(TransferJobV2::from_json(&nested_operation.to_string()).is_err());
    }

    #[test]
    fn hostile_refs_error_details_and_payload_metadata_are_rejected() {
        for value in [
            "",
            ".",
            "..",
            "../escape",
            "/tmp/file",
            "https://example.invalid",
        ] {
            assert!(OpaqueProfileRef::new(value).is_err(), "profile {value:?}");
            assert!(OpaqueResourceRef::new(value).is_err(), "resource {value:?}");
            assert!(OpaqueNodeRef::new(value).is_err(), "node {value:?}");
        }
        assert!(OpaqueProfileRef::new("password-prod").is_err());
        assert!(TransferError::new(
            TransferErrorCode::Internal,
            false,
            Some("failed at /etc/passwd".into()),
        )
        .is_err());
        assert!(
            TransferError::new(TransferErrorCode::Internal, false, Some("token=abc".into()),)
                .is_err()
        );

        let mut payload = PayloadRef::of_bytes(b"x");
        payload.content_type = Some("https://secret.invalid".into());
        let endpoint = TransferEndpoint::new(
            TransferLocation::Clipboard {
                profile: profile("clipboard-session"),
                payload,
            },
            local(),
        );
        let result = TransferJobV2::new(
            TransferId::new(),
            TransferKind::Clipboard,
            endpoint,
            TransferOperation::PublishClipboard,
            ChecksumPolicy::off(),
            None,
            1,
        );
        assert!(result.is_err());
    }

    #[test]
    fn progress_checksum_and_control_invariants_are_enforced() {
        let mut job = sample_job();
        job.progress.bytes_done = 9000;
        assert!(matches!(
            job.validate(),
            Err(TransferJobV2ValidationError::InvalidProgress { .. })
        ));

        let mut failed = sample_job();
        failed.state = TransferState::Failed;
        failed.progress.phase = TransferPhase::Failed;
        assert!(
            failed.validate().is_err(),
            "failed state needs a real error"
        );

        let mut completed = sample_job();
        completed.state = TransferState::Completed;
        completed.progress.phase = TransferPhase::Completed;
        completed.progress.bytes_done = 8192;
        assert!(completed.validate().is_ok());

        assert!(ChecksumPolicy::require("not-a-digest").is_err());
        assert!(TransferJobV2::can_control(&job, TransferControlV2::Pause));
        job.state = TransferState::Paused;
        job.progress.phase = TransferPhase::Paused;
        assert!(job.can_control(TransferControlV2::Resume));
        job.state = TransferState::Failed;
        job.progress.phase = TransferPhase::Failed;
        job.progress.attempt = 1;
        job.progress.error =
            Some(TransferError::new(TransferErrorCode::Timeout, true, None).expect("typed error"));
        assert!(job.can_control(TransferControlV2::Retry));
        assert!(!job.can_control(TransferControlV2::Cancel));
    }

    #[test]
    fn bounded_json_body_is_rejected_before_decode() {
        let body = vec![b' '; MAX_TRANSFER_JOB_V2_JSON_BYTES + 1];
        assert!(matches!(
            TransferJobV2::from_json_bytes(&body),
            Err(TransferJobV2DecodeError::BodyTooLarge { .. })
        ));
    }

    #[test]
    fn checksum_and_payload_digests_use_lower_hex_sha256() {
        let digest = sha256_hex(b"mesh");
        let policy = ChecksumPolicy::require(digest.clone()).expect("digest");
        assert_eq!(policy.expected_sha256_hex.as_deref(), Some(digest.as_str()));
        assert!(ChecksumPolicy::require(digest.to_ascii_uppercase()).is_err());
    }
}
