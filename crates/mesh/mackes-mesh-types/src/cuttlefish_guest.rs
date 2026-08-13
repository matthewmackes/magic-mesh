//! Closed, bounded wire contract between the host Cuttlefish adapter and the
//! guest-owned readiness relay.

use serde::{Deserialize, Serialize};

use crate::android_apps::{
    AndroidAppInventory, AndroidGuestLaunchOutcome, AndroidGuestLaunchRequest,
    AndroidImagePackageManifest,
};
use crate::android_provider::{AndroidVdiSource, CuttlefishVmTarget};

/// Current host/guest framing schema.
pub const CUTTLEFISH_GUEST_PROTOCOL_SCHEMA_VERSION: u16 = 1;
/// Maximum JSON frame accepted in either direction.
pub const CUTTLEFISH_GUEST_MAX_FRAME_BYTES: usize = 256 * 1024;

/// Closed operation accepted by the guest runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "op",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CuttlefishGuestOperation {
    /// Observe current package and display readiness.
    Observe,
    /// Launch one catalog-governed application intent.
    Launch(AndroidGuestLaunchRequest),
}

/// One correlated request from the host adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CuttlefishGuestRequest {
    pub schema_version: u16,
    pub request_id: String,
    pub target: CuttlefishVmTarget,
    pub catalog_digest: String,
    pub package_manifest: AndroidImagePackageManifest,
    pub generation: u64,
    pub operation: CuttlefishGuestOperation,
}

impl CuttlefishGuestRequest {
    /// Construct and validate an exact request.
    pub fn new(
        request_id: impl Into<String>,
        target: CuttlefishVmTarget,
        catalog_digest: impl Into<String>,
        package_manifest: AndroidImagePackageManifest,
        generation: u64,
        operation: CuttlefishGuestOperation,
    ) -> Result<Self, CuttlefishGuestContractError> {
        let request = Self {
            schema_version: CUTTLEFISH_GUEST_PROTOCOL_SCHEMA_VERSION,
            request_id: request_id.into(),
            target,
            catalog_digest: catalog_digest.into(),
            package_manifest,
            generation,
            operation,
        };
        request.validate()?;
        Ok(request)
    }

    /// Re-attest all identities before any guest effect.
    pub fn validate(&self) -> Result<(), CuttlefishGuestContractError> {
        if self.schema_version != CUTTLEFISH_GUEST_PROTOCOL_SCHEMA_VERSION {
            return Err(CuttlefishGuestContractError::UnsupportedSchema);
        }
        self.target
            .validate()
            .map_err(|_| CuttlefishGuestContractError::Target)?;
        self.package_manifest
            .validate()
            .map_err(|_| CuttlefishGuestContractError::Manifest)?;
        if self.generation == 0
            || !bounded_identity(&self.request_id)
            || !valid_digest(&self.catalog_digest)
            || self.package_manifest.image_provenance.image_id
                != self.target.image_provenance.image_id
            || self.package_manifest.image_provenance.image_digest
                != self.target.image_provenance.image_digest
            || self.package_manifest.image_provenance.source_revision
                != self.target.image_provenance.source_revision
            || self.package_manifest.image_provenance.catalog_revision
                != self.target.image_provenance.catalog_revision
        {
            return Err(CuttlefishGuestContractError::Identity);
        }
        if let CuttlefishGuestOperation::Launch(launch) = &self.operation {
            launch
                .validate()
                .map_err(|_| CuttlefishGuestContractError::Launch)?;
            let package = self
                .package_manifest
                .packages
                .iter()
                .find(|package| package.app == launch.app)
                .ok_or(CuttlefishGuestContractError::Launch)?;
            if launch.workload_id != self.target.vm_id.as_str()
                || package.package_id != launch.intent.package_id
            {
                return Err(CuttlefishGuestContractError::Launch);
            }
        }
        Ok(())
    }
}

/// One correlated response from the guest runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CuttlefishGuestResponse {
    pub schema_version: u16,
    pub request_id: String,
    pub target: CuttlefishVmTarget,
    pub catalog_digest: String,
    pub generation: u64,
    #[serde(default)]
    pub inventory: Option<AndroidAppInventory>,
    #[serde(default)]
    pub launch_outcome: Option<AndroidGuestLaunchOutcome>,
    #[serde(default)]
    pub vdi_source: Option<AndroidVdiSource>,
    #[serde(default)]
    pub cleanup_complete: bool,
}

impl CuttlefishGuestResponse {
    /// Construct the immutable correlation envelope for a request.
    #[must_use]
    pub fn correlated(request: &CuttlefishGuestRequest) -> Self {
        Self {
            schema_version: CUTTLEFISH_GUEST_PROTOCOL_SCHEMA_VERSION,
            request_id: request.request_id.clone(),
            target: request.target.clone(),
            catalog_digest: request.catalog_digest.clone(),
            generation: request.generation,
            inventory: None,
            launch_outcome: None,
            vdi_source: None,
            cleanup_complete: false,
        }
    }

    /// Validate only shape and exact request correlation. Freshness remains a
    /// host policy because it depends on the host's admission clock.
    pub fn validate_for(
        &self,
        request: &CuttlefishGuestRequest,
    ) -> Result<(), CuttlefishGuestContractError> {
        request.validate()?;
        if self.schema_version != CUTTLEFISH_GUEST_PROTOCOL_SCHEMA_VERSION
            || self.request_id != request.request_id
            || self.target != request.target
            || self.catalog_digest != request.catalog_digest
            || self.generation != request.generation
        {
            return Err(CuttlefishGuestContractError::Correlation);
        }
        match &request.operation {
            CuttlefishGuestOperation::Observe => {
                if self.inventory.is_none()
                    || self.vdi_source.is_none()
                    || self.launch_outcome.is_some()
                    || self.cleanup_complete
                {
                    return Err(CuttlefishGuestContractError::Shape);
                }
            }
            CuttlefishGuestOperation::Launch(_) => {
                if self.launch_outcome.is_none()
                    || self.inventory.is_some()
                    || self.vdi_source.is_some()
                    || self.cleanup_complete
                {
                    return Err(CuttlefishGuestContractError::Shape);
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CuttlefishGuestContractError {
    UnsupportedSchema,
    Target,
    Manifest,
    Identity,
    Launch,
    Correlation,
    Shape,
}

impl core::fmt::Display for CuttlefishGuestContractError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "invalid Cuttlefish guest contract: {self:?}")
    }
}

impl std::error::Error for CuttlefishGuestContractError {}

fn bounded_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        && value[7..].bytes().any(|byte| byte != b'0')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::android_apps::{
        AndroidGuestLaunchRequest, AndroidImagePackage, AndroidImageProvenance,
        AndroidPackageVersion, AospStarterApp,
    };
    use crate::android_provider::{CuttlefishImageProvenanceRef, CuttlefishVmId};

    const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn request() -> CuttlefishGuestRequest {
        let provenance =
            AndroidImageProvenance::new("image", DIGEST, "source-r1", "catalog-r1").unwrap();
        let manifest = AndroidImagePackageManifest::new(
            provenance,
            AospStarterApp::ALL
                .into_iter()
                .map(|app| {
                    AndroidImagePackage::for_app(app, AndroidPackageVersion::new("1.0", 1).unwrap())
                })
                .collect(),
        )
        .unwrap();
        let target = CuttlefishVmTarget::new(
            CuttlefishVmId::new("android-one").unwrap(),
            CuttlefishImageProvenanceRef::new("image", DIGEST, "source-r1", "catalog-r1").unwrap(),
        )
        .unwrap();
        CuttlefishGuestRequest::new(
            "launch-1",
            target,
            DIGEST,
            manifest,
            1,
            CuttlefishGuestOperation::Launch(
                AndroidGuestLaunchRequest::for_app(
                    "launch-1",
                    "android-one",
                    AospStarterApp::Browser,
                )
                .unwrap(),
            ),
        )
        .unwrap()
    }

    #[test]
    fn contract_rejects_cross_workload_launch_and_mixed_response_shape() {
        let mut hostile = request();
        let CuttlefishGuestOperation::Launch(launch) = &mut hostile.operation else {
            unreachable!()
        };
        launch.workload_id = "other-workload".into();
        assert_eq!(
            hostile.validate(),
            Err(CuttlefishGuestContractError::Launch)
        );

        let request = request();
        let mut response = CuttlefishGuestResponse::correlated(&request);
        response.launch_outcome = Some(AndroidGuestLaunchOutcome::Started);
        response.inventory = Some(AndroidAppInventory::pending("android-one"));
        assert_eq!(
            response.validate_for(&request),
            Err(CuttlefishGuestContractError::Shape)
        );
    }
}
