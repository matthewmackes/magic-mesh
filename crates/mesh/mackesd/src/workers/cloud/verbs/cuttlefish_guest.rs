//! Guest-owned Cuttlefish package/session transport.
//!
//! The host sends a closed typed operation over a workload-scoped Unix socket
//! exposed by the guest relay. It never constructs adb, package-manager, qemu,
//! intent, endpoint, or shell command lines.

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mackes_mesh_types::android_apps::{
    AndroidAppInventory, AndroidGuestLaunchOutcome, AndroidGuestLaunchRequest,
    AndroidImagePackageManifest,
};
use mackes_mesh_types::android_provider::{AndroidVdiSource, CuttlefishVmTarget};
use mackes_mesh_types::cuttlefish_guest::{
    CuttlefishGuestOperation as GuestOperation, CuttlefishGuestRequest as GuestRequest,
    CuttlefishGuestResponse as GuestResponse,
    CUTTLEFISH_GUEST_MAX_FRAME_BYTES as MAX_GUEST_FRAME_BYTES,
};

use super::cuttlefish::CuttlefishProviderError;

const GUEST_IO_TIMEOUT: Duration = Duration::from_secs(3);
const DEFAULT_GUEST_SOCKET_ROOT: &str = "/run/mackesd/cuttlefish-guest";
const GUEST_SOCKET_ROOT_ENV: &str = "MDE_CUTTLEFISH_GUEST_SOCKET_DIR";

/// Exact guest evidence returned from an observe operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GuestSnapshot {
    pub inventory: AndroidAppInventory,
    pub vdi_source: AndroidVdiSource,
}

/// Closed guest-agent operations consumed by the Cuttlefish provider client.
pub(super) trait CuttlefishGuestTransport: Send + Sync {
    fn observe(
        &self,
        request_id: &str,
        target: &CuttlefishVmTarget,
        catalog_digest: &str,
        package_manifest: &AndroidImagePackageManifest,
        generation: u64,
    ) -> Result<GuestSnapshot, CuttlefishProviderError>;

    fn launch(
        &self,
        request: &AndroidGuestLaunchRequest,
        target: &CuttlefishVmTarget,
        catalog_digest: &str,
        package_manifest: &AndroidImagePackageManifest,
        generation: u64,
    ) -> Result<AndroidGuestLaunchOutcome, CuttlefishProviderError>;
}

/// Production framed transport to the workload-scoped guest relay.
pub(super) struct UnixCuttlefishGuestTransport {
    socket_root: PathBuf,
}

impl UnixCuttlefishGuestTransport {
    pub(super) fn production() -> Self {
        Self {
            socket_root: std::env::var_os(GUEST_SOCKET_ROOT_ENV)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_GUEST_SOCKET_ROOT)),
        }
    }

    #[cfg(test)]
    fn at(socket_root: PathBuf) -> Self {
        Self { socket_root }
    }

    fn exchange(&self, request: GuestRequest) -> Result<GuestResponse, CuttlefishProviderError> {
        request
            .validate()
            .map_err(|_| CuttlefishProviderError::ProviderRejected)?;
        let socket = socket_path(&self.socket_root, request.target.vm_id.as_str())?;
        let mut stream = UnixStream::connect(&socket)
            .map_err(|_| CuttlefishProviderError::ProviderUnavailable)?;
        validate_connected_socket(&self.socket_root, &socket, &stream)?;
        stream
            .set_read_timeout(Some(GUEST_IO_TIMEOUT))
            .and_then(|()| stream.set_write_timeout(Some(GUEST_IO_TIMEOUT)))
            .map_err(|_| CuttlefishProviderError::ProviderUnavailable)?;
        let body =
            serde_json::to_vec(&request).map_err(|_| CuttlefishProviderError::ProviderRejected)?;
        if body.is_empty() || body.len() > MAX_GUEST_FRAME_BYTES {
            return Err(CuttlefishProviderError::ProviderRejected);
        }
        // Observe is a current-runtime proof, not permission to replay a
        // previously valid guest snapshot under the same outer-VM generation.
        // Capture the admission floor immediately before the request leaves
        // this process so both returned observations must belong to this
        // exchange (and therefore survive guest-relay restart honestly).
        let observation_not_before_unix_ms = now_unix_ms();
        let length = u32::try_from(body.len())
            .map_err(|_| CuttlefishProviderError::ProviderRejected)?
            .to_be_bytes();
        stream
            .write_all(&length)
            .and_then(|()| stream.write_all(&body))
            .and_then(|()| stream.flush())
            .map_err(|_| CuttlefishProviderError::ProviderUnavailable)?;

        let mut length = [0_u8; 4];
        stream
            .read_exact(&mut length)
            .map_err(|_| CuttlefishProviderError::ProviderUnavailable)?;
        let length = usize::try_from(u32::from_be_bytes(length))
            .map_err(|_| CuttlefishProviderError::ProviderRejected)?;
        if length == 0 || length > MAX_GUEST_FRAME_BYTES {
            return Err(CuttlefishProviderError::ProviderRejected);
        }
        let mut response = vec![0_u8; length];
        stream
            .read_exact(&mut response)
            .map_err(|_| CuttlefishProviderError::ProviderUnavailable)?;
        let response: GuestResponse = serde_json::from_slice(&response)
            .map_err(|_| CuttlefishProviderError::ProviderRejected)?;
        validate_response(&request, response, observation_not_before_unix_ms)
    }

    fn request(
        request_id: &str,
        target: &CuttlefishVmTarget,
        catalog_digest: &str,
        package_manifest: &AndroidImagePackageManifest,
        generation: u64,
        operation: GuestOperation,
    ) -> GuestRequest {
        GuestRequest::new(
            request_id,
            target.clone(),
            catalog_digest,
            package_manifest.clone(),
            generation,
            operation,
        )
        .expect("validated provider inputs must form a guest request")
    }
}

/// Bind a connected relay to the protected runtime directory and the kernel's
/// peer credentials before sending catalog, package, or lifecycle data.
///
/// Checking only `is_socket()` leaves the guest-readiness authority open to a
/// socket planted in a writable or symlink-substituted runtime directory.  The
/// post-connect checks also close the cross-uid path replacement race: the
/// server process must own both the protected directory and the socket inode.
fn validate_connected_socket(
    root: &Path,
    socket: &Path,
    stream: &UnixStream,
) -> Result<(), CuttlefishProviderError> {
    let canonical_root =
        fs::canonicalize(root).map_err(|_| CuttlefishProviderError::ProviderUnavailable)?;
    if canonical_root != root {
        return Err(CuttlefishProviderError::ProviderUnavailable);
    }
    let root_metadata =
        fs::symlink_metadata(root).map_err(|_| CuttlefishProviderError::ProviderUnavailable)?;
    if !root_metadata.file_type().is_dir() || root_metadata.permissions().mode() & 0o022 != 0 {
        return Err(CuttlefishProviderError::ProviderUnavailable);
    }

    let socket_metadata =
        fs::symlink_metadata(socket).map_err(|_| CuttlefishProviderError::ProviderUnavailable)?;
    if !socket_metadata.file_type().is_socket()
        || socket_metadata.permissions().mode() & 0o022 != 0
        || socket_metadata.uid() != root_metadata.uid()
    {
        return Err(CuttlefishProviderError::ProviderUnavailable);
    }
    let peer = rustix::net::sockopt::get_socket_peercred(stream)
        .map_err(|_| CuttlefishProviderError::ProviderUnavailable)?;
    if peer.uid.as_raw() != socket_metadata.uid() {
        return Err(CuttlefishProviderError::ProviderUnavailable);
    }
    Ok(())
}

impl CuttlefishGuestTransport for UnixCuttlefishGuestTransport {
    fn observe(
        &self,
        request_id: &str,
        target: &CuttlefishVmTarget,
        catalog_digest: &str,
        package_manifest: &AndroidImagePackageManifest,
        generation: u64,
    ) -> Result<GuestSnapshot, CuttlefishProviderError> {
        let response = self.exchange(Self::request(
            request_id,
            target,
            catalog_digest,
            package_manifest,
            generation,
            GuestOperation::Observe,
        ))?;
        let inventory = response
            .inventory
            .ok_or(CuttlefishProviderError::ProviderRejected)?;
        let vdi_source = response
            .vdi_source
            .ok_or(CuttlefishProviderError::ProviderRejected)?;
        Ok(GuestSnapshot {
            inventory,
            vdi_source,
        })
    }

    fn launch(
        &self,
        request: &AndroidGuestLaunchRequest,
        target: &CuttlefishVmTarget,
        catalog_digest: &str,
        package_manifest: &AndroidImagePackageManifest,
        generation: u64,
    ) -> Result<AndroidGuestLaunchOutcome, CuttlefishProviderError> {
        self.exchange(Self::request(
            &request.request_id,
            target,
            catalog_digest,
            package_manifest,
            generation,
            GuestOperation::Launch(request.clone()),
        ))?
        .launch_outcome
        .ok_or(CuttlefishProviderError::ProviderRejected)
    }
}

fn validate_response(
    request: &GuestRequest,
    response: GuestResponse,
    observation_not_before_unix_ms: u64,
) -> Result<GuestResponse, CuttlefishProviderError> {
    response
        .validate_for(request)
        .map_err(|_| CuttlefishProviderError::ProviderRejected)?;
    match &request.operation {
        GuestOperation::Observe => {
            let inventory = response
                .inventory
                .as_ref()
                .ok_or(CuttlefishProviderError::ProviderRejected)?;
            let now = now_unix_ms();
            inventory
                .validate_at(now)
                .map_err(CuttlefishProviderError::InventoryContract)?;
            let inventory_observed_at_unix_ms = inventory
                .observed_at_unix_ms
                .ok_or(CuttlefishProviderError::ProviderRejected)?;
            if inventory.workload_id != request.target.vm_id.as_str()
                || inventory.image_provenance.as_ref()
                    != Some(&request.package_manifest.image_provenance)
                || inventory.guest_boot_state
                    != mackes_mesh_types::android_apps::AndroidGuestBootState::Ready
                || inventory_observed_at_unix_ms < observation_not_before_unix_ms
            {
                return Err(CuttlefishProviderError::ProviderRejected);
            }
            let source = response
                .vdi_source
                .clone()
                .ok_or(CuttlefishProviderError::ProviderRejected)?
                .admitted_against(&request.target, &request.catalog_digest, request.generation)
                .map_err(CuttlefishProviderError::Contract)?;
            if source.observed_at_unix_ms < observation_not_before_unix_ms
                || source.observed_at_unix_ms > now
                || source.expires_at_unix_ms <= now
            {
                return Err(CuttlefishProviderError::ProviderRejected);
            }
            if response.launch_outcome.is_some() || response.cleanup_complete {
                return Err(CuttlefishProviderError::ProviderRejected);
            }
        }
        GuestOperation::Launch(_) => {
            if response.launch_outcome.is_none()
                || response.inventory.is_some()
                || response.vdi_source.is_some()
                || response.cleanup_complete
            {
                return Err(CuttlefishProviderError::ProviderRejected);
            }
        }
    }
    Ok(response)
}

fn socket_path(root: &Path, workload_id: &str) -> Result<PathBuf, CuttlefishProviderError> {
    if workload_id.is_empty()
        || workload_id.len() > 128
        || !workload_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(CuttlefishProviderError::InvalidWorkloadIdentity);
    }
    Ok(root.join(format!("{workload_id}.sock")))
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;
    use std::thread;

    use mackes_mesh_types::android_apps::{
        AndroidAppAvailability, AndroidAppInventoryEntry, AndroidAppReadiness,
        AndroidGuestBootState, AndroidImagePackage, AndroidImageProvenance, AndroidLaunchReadiness,
        AndroidLauncherResolvability, AndroidPackageVersion, AospStarterApp,
    };
    use mackes_mesh_types::android_provider::{
        AndroidVdiProtocol, CuttlefishImageProvenanceRef, CuttlefishVmId,
        ANDROID_VDI_SOURCE_SCHEMA_VERSION,
    };
    use mackes_mesh_types::cuttlefish_guest::CUTTLEFISH_GUEST_PROTOCOL_SCHEMA_VERSION as GUEST_PROTOCOL_SCHEMA_VERSION;

    use super::*;

    const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const CATALOG_DIGEST: &str =
        "sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";

    fn target() -> CuttlefishVmTarget {
        CuttlefishVmTarget::new(
            CuttlefishVmId::new("android-one").expect("VM id"),
            CuttlefishImageProvenanceRef::new("android-image", DIGEST, "source-r1", "catalog-r1")
                .expect("image provenance"),
        )
        .expect("target")
    }

    fn manifest() -> AndroidImagePackageManifest {
        let provenance =
            AndroidImageProvenance::new("android-image", DIGEST, "source-r1", "catalog-r1")
                .expect("provenance");
        let version = AndroidPackageVersion::new("1.0.0", 1).expect("version");
        AndroidImagePackageManifest::new(
            provenance,
            AospStarterApp::ALL
                .into_iter()
                .map(|app| AndroidImagePackage::for_app(app, version.clone()))
                .collect(),
        )
        .expect("manifest")
    }

    fn ready_inventory() -> AndroidAppInventory {
        let manifest = manifest();
        let entries = manifest
            .packages
            .iter()
            .map(|package| AndroidAppInventoryEntry {
                descriptor: package.app.descriptor(),
                availability: AndroidAppAvailability::Installed,
                package_version: Some(package.version.clone()),
                readiness: AndroidAppReadiness::Ready,
                launcher_resolvability: AndroidLauncherResolvability::Resolved,
                launch_readiness: AndroidLaunchReadiness::Ready,
                unavailable_reason: None,
            })
            .collect();
        let now = now_unix_ms();
        AndroidAppInventory::observed(
            "android-one",
            manifest.image_provenance,
            AndroidGuestBootState::Ready,
            now,
            0,
            entries,
        )
        .expect("ready inventory")
    }

    fn vdi_source() -> AndroidVdiSource {
        let now = now_unix_ms();
        AndroidVdiSource {
            schema_version: ANDROID_VDI_SOURCE_SCHEMA_VERSION,
            workload_id: "android-one".to_owned(),
            image_provenance: target().image_provenance,
            catalog_digest: CATALOG_DIGEST.to_owned(),
            generation: 7,
            protocol: AndroidVdiProtocol::WebRtc,
            mesh_host: "android-one.mesh".to_owned(),
            port: 8443,
            session_id: "session-7".to_owned(),
            observed_at_unix_ms: now,
            expires_at_unix_ms: now.saturating_add(60_000),
        }
    }

    fn read_request(stream: &mut UnixStream) -> GuestRequest {
        let mut length = [0_u8; 4];
        stream.read_exact(&mut length).expect("request length");
        let mut body = vec![0_u8; u32::from_be_bytes(length) as usize];
        stream.read_exact(&mut body).expect("request body");
        serde_json::from_slice(&body).expect("typed request")
    }

    fn write_response(stream: &mut UnixStream, response: &GuestResponse) {
        let body = serde_json::to_vec(response).expect("response JSON");
        stream
            .write_all(&(body.len() as u32).to_be_bytes())
            .expect("response length");
        stream.write_all(&body).expect("response body");
    }

    #[test]
    fn observe_constructs_closed_exact_contract_and_admits_ready_vdi() {
        let temporary = tempfile::tempdir().expect("socket root");
        let socket = temporary.path().join("android-one.sock");
        let listener = UnixListener::bind(&socket).expect("guest listener");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("guest connection");
            let request = read_request(&mut stream);
            assert_eq!(request.target, target());
            assert_eq!(request.package_manifest, manifest());
            assert_eq!(request.catalog_digest, CATALOG_DIGEST);
            assert_eq!(request.generation, 7);
            assert_eq!(request.operation, GuestOperation::Observe);
            write_response(
                &mut stream,
                &GuestResponse {
                    schema_version: GUEST_PROTOCOL_SCHEMA_VERSION,
                    request_id: request.request_id,
                    target: request.target,
                    catalog_digest: request.catalog_digest,
                    generation: request.generation,
                    inventory: Some(ready_inventory()),
                    launch_outcome: None,
                    vdi_source: Some(vdi_source()),
                    cleanup_complete: false,
                },
            );
        });
        let transport = UnixCuttlefishGuestTransport::at(temporary.path().to_path_buf());
        let snapshot = transport
            .observe("observe-7", &target(), CATALOG_DIGEST, &manifest(), 7)
            .expect("admitted guest snapshot");
        assert_eq!(
            snapshot.inventory.guest_boot_state,
            AndroidGuestBootState::Ready
        );
        assert_eq!(snapshot.vdi_source.session_id, "session-7");
        server.join().expect("guest server");
    }

    #[test]
    fn oversized_and_identity_drifted_guest_output_fail_closed() {
        let temporary = tempfile::tempdir().expect("socket root");
        let socket = temporary.path().join("android-one.sock");
        let listener = UnixListener::bind(&socket).expect("guest listener");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("guest connection");
            let _ = read_request(&mut stream);
            stream
                .write_all(
                    &u32::try_from(MAX_GUEST_FRAME_BYTES + 1)
                        .unwrap()
                        .to_be_bytes(),
                )
                .expect("hostile length");
        });
        let transport = UnixCuttlefishGuestTransport::at(temporary.path().to_path_buf());
        assert_eq!(
            transport.observe("observe-7", &target(), CATALOG_DIGEST, &manifest(), 7),
            Err(CuttlefishProviderError::ProviderRejected)
        );
        server.join().expect("guest server");

        let mut hostile = GuestRequest {
            schema_version: GUEST_PROTOCOL_SCHEMA_VERSION,
            request_id: "launch-7".to_owned(),
            target: target(),
            catalog_digest: CATALOG_DIGEST.to_owned(),
            package_manifest: manifest(),
            generation: 7,
            operation: GuestOperation::Launch(
                AndroidGuestLaunchRequest::for_app(
                    "launch-7",
                    "android-one",
                    AospStarterApp::Browser,
                )
                .expect("launch"),
            ),
        };
        hostile.target.image_provenance.catalog_revision = "drifted".to_owned();
        assert!(hostile.validate().is_err());
    }

    #[test]
    fn future_inventory_observation_cannot_invent_guest_readiness() {
        let request = GuestRequest {
            schema_version: GUEST_PROTOCOL_SCHEMA_VERSION,
            request_id: "observe-7".to_owned(),
            target: target(),
            catalog_digest: CATALOG_DIGEST.to_owned(),
            package_manifest: manifest(),
            generation: 7,
            operation: GuestOperation::Observe,
        };
        let mut inventory = ready_inventory();
        inventory.observed_at_unix_ms = Some(now_unix_ms().saturating_add(60_000));
        inventory.observation_age_ms = Some(0);
        let response = GuestResponse {
            schema_version: GUEST_PROTOCOL_SCHEMA_VERSION,
            request_id: request.request_id.clone(),
            target: request.target.clone(),
            catalog_digest: request.catalog_digest.clone(),
            generation: request.generation,
            inventory: Some(inventory),
            launch_outcome: None,
            vdi_source: Some(vdi_source()),
            cleanup_complete: false,
        };

        assert!(matches!(
            validate_response(&request, response, now_unix_ms()),
            Err(CuttlefishProviderError::InventoryContract(_))
        ));
    }

    #[test]
    fn pre_restart_inventory_cannot_authorize_the_current_guest_exchange() {
        let temporary = tempfile::tempdir().expect("socket root");
        let socket = temporary.path().join("android-one.sock");
        let listener = UnixListener::bind(&socket).expect("guest listener");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("guest connection");
            let request = read_request(&mut stream);
            let mut stale_inventory = ready_inventory();
            stale_inventory.observed_at_unix_ms = Some(now_unix_ms().saturating_sub(60_000).max(1));
            stale_inventory.observation_age_ms = Some(0);
            write_response(
                &mut stream,
                &GuestResponse {
                    schema_version: GUEST_PROTOCOL_SCHEMA_VERSION,
                    request_id: request.request_id,
                    target: request.target,
                    catalog_digest: request.catalog_digest,
                    generation: request.generation,
                    inventory: Some(stale_inventory),
                    launch_outcome: None,
                    vdi_source: Some(vdi_source()),
                    cleanup_complete: false,
                },
            );
        });

        let transport = UnixCuttlefishGuestTransport::at(temporary.path().to_path_buf());
        assert_eq!(
            transport.observe("observe-7", &target(), CATALOG_DIGEST, &manifest(), 7),
            Err(CuttlefishProviderError::ProviderRejected),
            "a fresh envelope must not relabel pre-restart package readiness"
        );
        server.join().expect("guest server");
    }

    #[test]
    fn transport_rejects_writable_guest_relay_before_sending_authority_data() {
        let temporary = tempfile::tempdir().expect("socket root");
        let socket = temporary.path().join("android-one.sock");
        let listener = UnixListener::bind(&socket).expect("guest listener");
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o777))
            .expect("make hostile relay writable");

        let transport = UnixCuttlefishGuestTransport::at(temporary.path().to_path_buf());
        assert_eq!(
            transport.observe("observe-7", &target(), CATALOG_DIGEST, &manifest(), 7),
            Err(CuttlefishProviderError::ProviderUnavailable)
        );

        let (mut intercepted, _) = listener.accept().expect("intercepted connection");
        let mut first_byte = [0_u8; 1];
        assert_eq!(
            intercepted
                .read(&mut first_byte)
                .expect("client closes without a request"),
            0,
            "an unauthenticated relay must receive no governed request bytes"
        );
    }
}
