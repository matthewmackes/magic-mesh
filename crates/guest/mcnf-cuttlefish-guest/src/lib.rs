//! Production Cuttlefish guest runtime. The relay owns the authenticated Unix
//! boundary; the agent owns bounded `adb` observation and launch effects.

use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mackes_mesh_types::android_apps::{
    AndroidAppAvailability, AndroidAppInventory, AndroidAppInventoryEntry, AndroidAppReadiness,
    AndroidGuestBootState, AndroidGuestLaunchOutcome, AndroidImagePackage, AndroidLaunchReadiness,
    AndroidLauncherResolvability,
};
use mackes_mesh_types::android_provider::{
    AndroidVdiProtocol, AndroidVdiSource, ANDROID_VDI_SOURCE_SCHEMA_VERSION,
};
use mackes_mesh_types::cuttlefish_guest::{
    CuttlefishGuestOperation, CuttlefishGuestRequest, CuttlefishGuestResponse,
    CUTTLEFISH_GUEST_MAX_FRAME_BYTES,
};

/// Immutable provenance marker retained in both ELF files after governed Cargo stripping.
#[used]
pub static BUILD_SOURCE_MARKER: &str =
    concat!("MCNF_SOURCE_REVISION=", env!("MCNF_GUEST_SOURCE_REVISION"));

pub const BUILD_SOURCE_REVISION: &str = env!("MCNF_GUEST_SOURCE_REVISION");
const MAX_TOOL_OUTPUT_BYTES: usize = 256 * 1024;
const TOOL_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug)]
pub enum RuntimeError {
    InvalidContract,
    InvalidConfiguration,
    UnauthenticatedPeer,
    ToolUnavailable,
    ToolRejected,
    BoundExceeded,
    Io(io::Error),
}

impl core::fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "Cuttlefish guest I/O failed: {error}"),
            other => write!(formatter, "Cuttlefish guest runtime refused: {other:?}"),
        }
    }
}

impl std::error::Error for RuntimeError {}

impl From<io::Error> for RuntimeError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub adb: std::path::PathBuf,
    pub mesh_host: String,
    pub webrtc_port: u16,
    pub session_id: String,
}

impl AgentConfig {
    pub fn validate(&self) -> Result<(), RuntimeError> {
        validate_executable(&self.adb, None)?;
        if self.webrtc_port == 0
            || !valid_identity(&self.mesh_host)
            || !valid_identity(&self.session_id)
        {
            return Err(RuntimeError::InvalidConfiguration);
        }
        Ok(())
    }
}

pub trait AndroidRuntimeBackend {
    fn attest_package(&self, package: &AndroidImagePackage) -> Result<bool, RuntimeError>;
    fn launch(
        &self,
        package: &AndroidImagePackage,
    ) -> Result<AndroidGuestLaunchOutcome, RuntimeError>;
}

pub struct AdbBackend<'a> {
    pub executable: &'a Path,
}

impl AdbBackend<'_> {
    fn output(&self, arguments: &[&str]) -> Result<String, RuntimeError> {
        let mut command = Command::new(self.executable);
        command
            .args(arguments)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null());
        let output = run_command_bounded(&mut command)?;
        if output.stdout.len().saturating_add(output.stderr.len()) > MAX_TOOL_OUTPUT_BYTES {
            return Err(RuntimeError::BoundExceeded);
        }
        if !output.status.success() {
            return Err(RuntimeError::ToolRejected);
        }
        String::from_utf8(output.stdout).map_err(|_| RuntimeError::ToolRejected)
    }

    fn package_id(package: &AndroidImagePackage) -> &str {
        package.package_id.as_str()
    }
}

impl AndroidRuntimeBackend for AdbBackend<'_> {
    fn attest_package(&self, package: &AndroidImagePackage) -> Result<bool, RuntimeError> {
        let id = Self::package_id(package);
        let dump = self.output(&["shell", "dumpsys", "package", id])?;
        let version_name = format!("versionName={}", package.version.version_name);
        let version_code = format!("versionCode={}", package.version.version_code);
        if !dump.lines().any(|line| line.trim() == version_name)
            || !dump
                .lines()
                .any(|line| line.trim_start().starts_with(&version_code))
        {
            return Ok(false);
        }
        let resolved = self.output(&[
            "shell",
            "cmd",
            "package",
            "resolve-activity",
            "--brief",
            "-a",
            "android.intent.action.MAIN",
            "-c",
            "android.intent.category.LAUNCHER",
            id,
        ])?;
        Ok(resolved
            .lines()
            .any(|line| line.trim().starts_with(&format!("{id}/"))))
    }

    fn launch(
        &self,
        package: &AndroidImagePackage,
    ) -> Result<AndroidGuestLaunchOutcome, RuntimeError> {
        if !self.attest_package(package)? {
            return Ok(AndroidGuestLaunchOutcome::Unavailable);
        }
        self.output(&[
            "shell",
            "am",
            "start",
            "-W",
            "-a",
            "android.intent.action.MAIN",
            "-c",
            "android.intent.category.LAUNCHER",
            "-p",
            Self::package_id(package),
        ])?;
        Ok(AndroidGuestLaunchOutcome::Started)
    }
}

pub fn handle_agent_request(
    request: &CuttlefishGuestRequest,
    backend: &dyn AndroidRuntimeBackend,
    config: &AgentConfig,
) -> Result<CuttlefishGuestResponse, RuntimeError> {
    request
        .validate()
        .map_err(|_| RuntimeError::InvalidContract)?;
    config.validate()?;
    let mut response = CuttlefishGuestResponse::correlated(request);
    match &request.operation {
        CuttlefishGuestOperation::Observe => {
            let mut entries = Vec::with_capacity(request.package_manifest.packages.len());
            for package in &request.package_manifest.packages {
                if !backend.attest_package(package)? {
                    return Err(RuntimeError::ToolRejected);
                }
                entries.push(AndroidAppInventoryEntry {
                    descriptor: package.app.descriptor(),
                    availability: AndroidAppAvailability::Installed,
                    package_version: Some(package.version.clone()),
                    readiness: AndroidAppReadiness::Ready,
                    launcher_resolvability: AndroidLauncherResolvability::Resolved,
                    launch_readiness: AndroidLaunchReadiness::Ready,
                    unavailable_reason: None,
                });
            }
            let now = now_unix_ms();
            response.inventory = Some(
                AndroidAppInventory::observed(
                    request.target.vm_id.as_str(),
                    request.package_manifest.image_provenance.clone(),
                    AndroidGuestBootState::Ready,
                    now,
                    0,
                    entries,
                )
                .map_err(|_| RuntimeError::InvalidContract)?,
            );
            response.vdi_source = Some(AndroidVdiSource {
                schema_version: ANDROID_VDI_SOURCE_SCHEMA_VERSION,
                workload_id: request.target.vm_id.as_str().to_owned(),
                image_provenance: request.target.image_provenance.clone(),
                catalog_digest: request.catalog_digest.clone(),
                generation: request.generation,
                protocol: AndroidVdiProtocol::WebRtc,
                mesh_host: config.mesh_host.clone(),
                port: config.webrtc_port,
                session_id: config.session_id.clone(),
                observed_at_unix_ms: now,
                expires_at_unix_ms: now.saturating_add(60_000),
            });
        }
        CuttlefishGuestOperation::Launch(launch) => {
            let package = request
                .package_manifest
                .packages
                .iter()
                .find(|package| package.app == launch.app)
                .ok_or(RuntimeError::InvalidContract)?;
            response.launch_outcome = Some(backend.launch(package)?);
        }
    }
    response
        .validate_for(request)
        .map_err(|_| RuntimeError::InvalidContract)?;
    Ok(response)
}

pub fn read_json_bounded(reader: &mut impl Read) -> Result<CuttlefishGuestRequest, RuntimeError> {
    let mut body = Vec::new();
    reader
        .take((CUTTLEFISH_GUEST_MAX_FRAME_BYTES + 1) as u64)
        .read_to_end(&mut body)?;
    if body.is_empty() || body.len() > CUTTLEFISH_GUEST_MAX_FRAME_BYTES {
        return Err(RuntimeError::BoundExceeded);
    }
    let request: CuttlefishGuestRequest =
        serde_json::from_slice(&body).map_err(|_| RuntimeError::InvalidContract)?;
    request
        .validate()
        .map_err(|_| RuntimeError::InvalidContract)?;
    Ok(request)
}

pub fn read_framed(stream: &mut UnixStream) -> Result<CuttlefishGuestRequest, RuntimeError> {
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length)?;
    let length =
        usize::try_from(u32::from_be_bytes(length)).map_err(|_| RuntimeError::BoundExceeded)?;
    if length == 0 || length > CUTTLEFISH_GUEST_MAX_FRAME_BYTES {
        return Err(RuntimeError::BoundExceeded);
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body)?;
    let request: CuttlefishGuestRequest =
        serde_json::from_slice(&body).map_err(|_| RuntimeError::InvalidContract)?;
    request
        .validate()
        .map_err(|_| RuntimeError::InvalidContract)?;
    Ok(request)
}

pub fn write_framed(
    stream: &mut UnixStream,
    response: &CuttlefishGuestResponse,
) -> Result<(), RuntimeError> {
    let body = serde_json::to_vec(response).map_err(|_| RuntimeError::InvalidContract)?;
    if body.is_empty() || body.len() > CUTTLEFISH_GUEST_MAX_FRAME_BYTES {
        return Err(RuntimeError::BoundExceeded);
    }
    stream.write_all(
        &u32::try_from(body.len())
            .map_err(|_| RuntimeError::BoundExceeded)?
            .to_be_bytes(),
    )?;
    stream.write_all(&body)?;
    stream.flush()?;
    Ok(())
}

pub fn validate_peer(stream: &UnixStream, expected_uid: u32) -> Result<(), RuntimeError> {
    let peer = rustix::net::sockopt::get_socket_peercred(stream)
        .map_err(|_| RuntimeError::UnauthenticatedPeer)?;
    if peer.uid.as_raw() != expected_uid {
        return Err(RuntimeError::UnauthenticatedPeer);
    }
    Ok(())
}

pub fn validate_executable(
    path: &Path,
    expected_revision: Option<&str>,
) -> Result<(), RuntimeError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| RuntimeError::InvalidConfiguration)?;
    if !metadata.file_type().is_file()
        || metadata.nlink() != 1
        || metadata.permissions().mode() & 0o022 != 0
        || (metadata.uid() != 0 && metadata.uid() != rustix::process::geteuid().as_raw())
    {
        return Err(RuntimeError::InvalidConfiguration);
    }
    if let Some(revision) = expected_revision {
        let mut command = Command::new(path);
        command.arg("--build-identity").env_clear();
        let output =
            run_command_bounded(&mut command).map_err(|_| RuntimeError::InvalidConfiguration)?;
        if !output.status.success() || output.stdout != format!("{revision}\n").as_bytes() {
            return Err(RuntimeError::InvalidConfiguration);
        }
    }
    Ok(())
}

pub fn invoke_agent(
    path: &Path,
    request: &CuttlefishGuestRequest,
    arguments: &[String],
) -> Result<CuttlefishGuestResponse, RuntimeError> {
    validate_executable(path, Some(BUILD_SOURCE_REVISION))?;
    let body = serde_json::to_vec(request).map_err(|_| RuntimeError::InvalidContract)?;
    let mut child = Command::new(path)
        .arg("--stdio")
        .args(arguments)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| RuntimeError::ToolUnavailable)?;
    child
        .stdin
        .take()
        .ok_or(RuntimeError::ToolUnavailable)?
        .write_all(&body)?;
    let output = wait_child_bounded(child)?;
    if !output.status.success() || output.stdout.len() > CUTTLEFISH_GUEST_MAX_FRAME_BYTES {
        return Err(RuntimeError::ToolRejected);
    }
    let response: CuttlefishGuestResponse =
        serde_json::from_slice(&output.stdout).map_err(|_| RuntimeError::InvalidContract)?;
    response
        .validate_for(request)
        .map_err(|_| RuntimeError::InvalidContract)?;
    Ok(response)
}

fn valid_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':'))
}

fn run_command_bounded(command: &mut Command) -> Result<std::process::Output, RuntimeError> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = command.spawn().map_err(|_| RuntimeError::ToolUnavailable)?;
    wait_child_bounded(child)
}

fn wait_child_bounded(
    mut child: std::process::Child,
) -> Result<std::process::Output, RuntimeError> {
    let deadline = Instant::now() + TOOL_TIMEOUT;
    loop {
        match child
            .try_wait()
            .map_err(|_| RuntimeError::ToolUnavailable)?
        {
            Some(_) => {
                return child
                    .wait_with_output()
                    .map_err(|_| RuntimeError::ToolUnavailable)
            }
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RuntimeError::ToolUnavailable);
            }
        }
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| u64::try_from(value.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::android_apps::{
        AndroidGuestLaunchRequest, AndroidImagePackageManifest, AndroidImageProvenance,
        AndroidPackageVersion, AospStarterApp,
    };
    use mackes_mesh_types::android_provider::{
        CuttlefishImageProvenanceRef, CuttlefishVmId, CuttlefishVmTarget,
    };
    use mackes_mesh_types::cuttlefish_guest::CuttlefishGuestOperation;
    use std::os::unix::net::UnixStream;
    use std::process::Command;

    const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    struct Backend {
        reject: bool,
    }
    impl AndroidRuntimeBackend for Backend {
        fn attest_package(&self, _: &AndroidImagePackage) -> Result<bool, RuntimeError> {
            Ok(!self.reject)
        }
        fn launch(
            &self,
            _: &AndroidImagePackage,
        ) -> Result<AndroidGuestLaunchOutcome, RuntimeError> {
            Ok(if self.reject {
                AndroidGuestLaunchOutcome::Unavailable
            } else {
                AndroidGuestLaunchOutcome::Started
            })
        }
    }
    fn request(operation: CuttlefishGuestOperation) -> CuttlefishGuestRequest {
        let manifest = AndroidImagePackageManifest::new(
            AndroidImageProvenance::new("image", DIGEST, "source-r1", "catalog-r1").unwrap(),
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
        CuttlefishGuestRequest::new("request-1", target, DIGEST, manifest, 7, operation).unwrap()
    }
    fn config() -> AgentConfig {
        AgentConfig {
            adb: "/bin/true".into(),
            mesh_host: "android-one.mesh".into(),
            webrtc_port: 8443,
            session_id: "session-7".into(),
        }
    }

    #[test]
    fn real_agent_contract_refuses_partial_inventory_and_cross_workload_launch() {
        let observe = request(CuttlefishGuestOperation::Observe);
        assert!(handle_agent_request(&observe, &Backend { reject: false }, &config()).is_ok());
        assert!(handle_agent_request(&observe, &Backend { reject: true }, &config()).is_err());
        let mut launch =
            AndroidGuestLaunchRequest::for_app("request-1", "android-one", AospStarterApp::Browser)
                .unwrap();
        launch.workload_id = "other".into();
        let mut hostile = request(CuttlefishGuestOperation::Observe);
        hostile.operation = CuttlefishGuestOperation::Launch(launch);
        assert!(handle_agent_request(&hostile, &Backend { reject: false }, &config()).is_err());
    }

    #[test]
    fn bounded_reader_rejects_oversized_or_unknown_protocol() {
        let oversized = vec![b'x'; CUTTLEFISH_GUEST_MAX_FRAME_BYTES + 1];
        assert!(matches!(
            read_json_bounded(&mut oversized.as_slice()),
            Err(RuntimeError::BoundExceeded)
        ));
        assert!(matches!(
            read_json_bounded(&mut br#"{"schema_version":999}"#.as_slice()),
            Err(RuntimeError::InvalidContract)
        ));
    }

    #[test]
    fn relay_peer_identity_and_tool_timeout_fail_closed() {
        let (server, _client) = UnixStream::pair().unwrap();
        let uid = rustix::process::getuid().as_raw();
        assert!(validate_peer(&server, uid).is_ok());
        assert!(matches!(
            validate_peer(&server, uid.saturating_add(1)),
            Err(RuntimeError::UnauthenticatedPeer)
        ));

        let mut sleeper = Command::new("/bin/sh");
        sleeper.arg("-c").arg("sleep 30");
        let started = Instant::now();
        assert!(matches!(
            run_command_bounded(&mut sleeper),
            Err(RuntimeError::ToolUnavailable)
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
