//! The typed adapter between the strict V2 transfer contract and the existing
//! Files-side read projection.
//!
//! [`TransferJobView`] is intentionally only a mirror of the authoritative
//! WL-FUNC-006 ledger. This module does not create another ledger or retain V2
//! endpoint data. It admits a validated [`TransferJobV2`] only when the
//! existing view can represent the job without losing the file identity or
//! changing its direction/status semantics.

use std::fmt;

use crate::ids::FileRefId;
use crate::read_model::TransferJobView;
use crate::value::{TransferDirection, TransferMethod};
use crate::{
    TransferJobV2, TransferJobV2ValidationError, TransferKind, TransferLocation, TransferOperation,
};

/// Why a V2 job cannot be admitted into the legacy Files-side projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferLedgerAdmissionError {
    /// The V2 contract itself is invalid and must not reach the ledger seam.
    InvalidV2(TransferJobV2ValidationError),
    /// The V2 executor family has no lossless legacy method representation.
    UnsupportedKind(TransferKind),
    /// The validated V2 operation has no legacy direction/status projection.
    UnsupportedOperation {
        /// V2 executor family whose operation is not representable.
        kind: TransferKind,
    },
    /// The side selected by the transfer direction is not a typed Files object.
    MissingFileReference {
        /// Direction whose endpoint side lacked a typed file reference.
        direction: TransferDirection,
    },
}

impl fmt::Display for TransferLedgerAdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidV2(error) => write!(formatter, "V2 transfer rejected: {error}"),
            Self::UnsupportedKind(kind) => write!(
                formatter,
                "V2 transfer kind {} has no legacy Files projection",
                kind.as_str()
            ),
            Self::UnsupportedOperation { kind } => write!(
                formatter,
                "V2 transfer operation for {} has no legacy Files projection",
                kind.as_str()
            ),
            Self::MissingFileReference { direction } => write!(
                formatter,
                "V2 transfer has no typed Files object on the {direction:?} side"
            ),
        }
    }
}

impl std::error::Error for TransferLedgerAdmissionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidV2(error) => Some(error),
            Self::UnsupportedKind(_)
            | Self::UnsupportedOperation { .. }
            | Self::MissingFileReference { .. } => None,
        }
    }
}

impl From<TransferJobV2ValidationError> for TransferLedgerAdmissionError {
    fn from(error: TransferJobV2ValidationError) -> Self {
        Self::InvalidV2(error)
    }
}

/// Admit one validated V2 job into the existing Files-side read projection.
///
/// The returned [`TransferJobView`] is a bounded projection, not a replacement
/// for the V2 job or the WL-FUNC-006 ledger. Endpoint/profile/resource data,
/// operation options, checksum policy, bandwidth limits, attempts, and typed
/// errors remain owned by their existing contracts and are deliberately not
/// copied into the legacy row.
///
/// # Errors
///
/// Returns an error when V2 validation fails, the executor family or operation
/// is not representable by the legacy view, or the direction-facing endpoint
/// does not carry a typed [`FileRefId`]. Commands, paths, URLs, and secrets
/// cannot enter this helper because it accepts only the validated V2 types.
pub fn admit_v2_job(job: &TransferJobV2) -> Result<TransferJobView, TransferLedgerAdmissionError> {
    job.validate()?;

    let method = legacy_method(job.kind)?;
    let direction = legacy_direction(job.kind, &job.operation)?;
    let file = file_on_direction_side(job, direction)
        .ok_or(TransferLedgerAdmissionError::MissingFileReference { direction })?;

    Ok(TransferJobView {
        transfer: job.transfer,
        file,
        method,
        direction,
        state: job.state,
        moved: job.progress.bytes_done,
        // The legacy view uses zero for an honestly unknown total.
        total: job.progress.total_bytes.unwrap_or(0),
    })
}

impl TryFrom<&TransferJobV2> for TransferJobView {
    type Error = TransferLedgerAdmissionError;

    fn try_from(job: &TransferJobV2) -> Result<Self, Self::Error> {
        admit_v2_job(job)
    }
}

const fn legacy_method(kind: TransferKind) -> Result<TransferMethod, TransferLedgerAdmissionError> {
    match kind {
        TransferKind::Mesh => Ok(TransferMethod::Node),
        TransferKind::Rsync => Ok(TransferMethod::Rsync),
        TransferKind::Sftp => Ok(TransferMethod::Sftp),
        TransferKind::Http => Ok(TransferMethod::Http),
        TransferKind::Scrape => Ok(TransferMethod::BrowserDownload),
        unsupported => Err(TransferLedgerAdmissionError::UnsupportedKind(unsupported)),
    }
}

const fn legacy_direction(
    kind: TransferKind,
    operation: &TransferOperation,
) -> Result<TransferDirection, TransferLedgerAdmissionError> {
    match (kind, operation) {
        (
            TransferKind::Mesh | TransferKind::Rsync | TransferKind::Sftp,
            TransferOperation::Copy { direction } | TransferOperation::Sync { direction, .. },
        ) => Ok(*direction),
        (TransferKind::Sftp | TransferKind::Http, TransferOperation::Download)
        | (TransferKind::Scrape, TransferOperation::Scrape { .. }) => {
            Ok(TransferDirection::Inbound)
        }
        (TransferKind::Sftp, TransferOperation::Upload) => Ok(TransferDirection::Outbound),
        (kind, _) => Err(TransferLedgerAdmissionError::UnsupportedOperation { kind }),
    }
}

const fn file_on_direction_side(job: &TransferJobV2, direction: TransferDirection) -> Option<FileRefId> {
    let location = match direction {
        TransferDirection::Inbound => &job.endpoint.destination,
        TransferDirection::Outbound => &job.endpoint.source,
    };
    match location {
        TransferLocation::Local { object } | TransferLocation::Mesh { object, .. } => Some(*object),
        TransferLocation::Rsync { .. }
        | TransferLocation::Sftp { .. }
        | TransferLocation::Http { .. }
        | TransferLocation::Scrape { .. }
        | TransferLocation::Multipart { .. }
        | TransferLocation::Clipboard { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ChecksumPolicy, OpaqueNodeRef, OpaqueProfileRef, OpaqueResourceRef, TransferEndpoint,
        TransferPhase, TransferProgress, TRANSFER_JOB_V2_SCHEMA_VERSION,
    };

    fn node(name: &str) -> OpaqueNodeRef {
        OpaqueNodeRef::new(name).expect("safe node ref")
    }

    fn profile(name: &str) -> OpaqueProfileRef {
        OpaqueProfileRef::new(name).expect("safe profile ref")
    }

    fn resource(name: &str) -> OpaqueResourceRef {
        OpaqueResourceRef::new(name).expect("safe resource ref")
    }

    fn mesh_job(direction: TransferDirection) -> TransferJobV2 {
        let file = crate::FileRefId::new();
        let remote_file = crate::FileRefId::new();
        let endpoint = match direction {
            TransferDirection::Inbound => TransferEndpoint::new(
                TransferLocation::Mesh {
                    node: node("peer-1"),
                    object: remote_file,
                },
                TransferLocation::Local { object: file },
            ),
            TransferDirection::Outbound => TransferEndpoint::new(
                TransferLocation::Local { object: file },
                TransferLocation::Mesh {
                    node: node("peer-1"),
                    object: remote_file,
                },
            ),
        };
        TransferJobV2 {
            schema_version: TRANSFER_JOB_V2_SCHEMA_VERSION,
            transfer: crate::TransferId::new(),
            kind: TransferKind::Mesh,
            endpoint,
            operation: TransferOperation::Copy { direction },
            state: crate::TransferState::Active,
            progress: TransferProgress {
                bytes_done: 512,
                total_bytes: Some(1024),
                bytes_per_second: Some(128),
                phase: TransferPhase::Transferring,
                attempt: 1,
                error: None,
            },
            checksum: ChecksumPolicy::off(),
            bandwidth_limit_bytes_per_second: None,
            created_unix_ms: 10,
            updated_unix_ms: 20,
        }
        .admitted()
        .expect("mesh fixture is valid")
    }

    #[test]
    fn mesh_v2_job_projects_to_the_existing_legacy_view() {
        let job = mesh_job(TransferDirection::Outbound);
        let view = admit_v2_job(&job).expect("representable mesh job");

        assert_eq!(view.transfer, job.transfer);
        assert_eq!(view.method, TransferMethod::Node);
        assert_eq!(view.direction, TransferDirection::Outbound);
        assert_eq!(view.state, crate::TransferState::Active);
        assert_eq!(view.moved, 512);
        assert_eq!(view.total, 1024);
        assert_eq!(
            view.file,
            match job.endpoint.source {
                TransferLocation::Local { object } => object,
                _ => unreachable!("fixture source is local"),
            }
        );
    }

    #[test]
    fn download_and_unknown_total_keep_legacy_direction_and_zero_total() {
        let file = crate::FileRefId::new();
        let mut job = TransferJobV2::new(
            crate::TransferId::new(),
            TransferKind::Http,
            TransferEndpoint::new(
                TransferLocation::Http {
                    profile: profile("public-http"),
                    resource: resource("object-1"),
                },
                TransferLocation::Local { object: file },
            ),
            TransferOperation::Download,
            ChecksumPolicy::off(),
            None,
            100,
        )
        .expect("HTTP fixture is valid");
        job.state = crate::TransferState::Active;
        job.progress = TransferProgress {
            bytes_done: 7,
            total_bytes: None,
            bytes_per_second: None,
            phase: TransferPhase::Transferring,
            attempt: 1,
            error: None,
        };
        job.updated_unix_ms = 101;
        let view = TransferJobView::try_from(&job).expect("HTTP download is representable");

        assert_eq!(view.method, TransferMethod::Http);
        assert_eq!(view.direction, TransferDirection::Inbound);
        assert_eq!(view.file, file);
        assert_eq!(view.moved, 7);
        assert_eq!(view.total, 0, "legacy zero means total is honestly unknown");
    }

    #[test]
    fn invalid_v2_is_rejected_before_legacy_projection() {
        let mut job = mesh_job(TransferDirection::Inbound);
        job.schema_version = TRANSFER_JOB_V2_SCHEMA_VERSION - 1;

        assert!(matches!(
            admit_v2_job(&job),
            Err(TransferLedgerAdmissionError::InvalidV2(
                TransferJobV2ValidationError::UnsupportedSchema { .. }
            ))
        ));
    }

    #[test]
    fn unsupported_v2_kinds_fail_closed() {
        let file = crate::FileRefId::new();
        let job = TransferJobV2::new(
            crate::TransferId::new(),
            TransferKind::Local,
            TransferEndpoint::new(
                TransferLocation::Local { object: file },
                TransferLocation::Local {
                    object: crate::FileRefId::new(),
                },
            ),
            TransferOperation::Copy {
                direction: TransferDirection::Outbound,
            },
            ChecksumPolicy::off(),
            None,
            100,
        )
        .expect("V2 local fixture is valid");

        assert!(matches!(
            admit_v2_job(&job),
            Err(TransferLedgerAdmissionError::UnsupportedKind(
                TransferKind::Local
            ))
        ));
    }

    #[test]
    fn missing_direction_side_file_is_rejected() {
        let job = TransferJobV2::new(
            crate::TransferId::new(),
            TransferKind::Sftp,
            TransferEndpoint::new(
                TransferLocation::Sftp {
                    profile: profile("sftp-profile"),
                    object: resource("source-1"),
                },
                TransferLocation::Sftp {
                    profile: profile("sftp-profile"),
                    object: resource("destination-1"),
                },
            ),
            TransferOperation::Download,
            ChecksumPolicy::off(),
            None,
            100,
        )
        .expect("V2 SFTP fixture is valid");

        assert!(matches!(
            admit_v2_job(&job),
            Err(TransferLedgerAdmissionError::MissingFileReference {
                direction: TransferDirection::Inbound
            })
        ));
    }
}
