//! Heartbeat + link telemetry (Phase 12.6.1 + 12.6.2).
//!
//! Per the 12.6.1 lock, every peer's `mackesd` writes health + agent
//! version + last-applied revision into its local
//! `observed_telemetry` table AND copies the row into
//! `~/QNM-Shared/<peer>/mackesd/heartbeat.json` (the shared
//! mesh-FS, the only "transport" without a networked API). The
//! Host's reconciler aggregates the per-peer files on its tick.
//!
//! Per 12.6.2, link telemetry (latency + packet loss + throughput
//! per peer-pair) lands at `~/QNM-Shared/<peer>/mackesd/links.json`
//! every 30 s. Aggregated per-link in `topology_link_health`.

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::path::{Path, PathBuf};

const ACTIVE_NEBULA_CERT: &str = "/etc/nebula/identity/current/host.crt";
const ACTIVE_NEBULA_CURRENT: &str = "/etc/nebula/identity/current";
const LEGACY_NEBULA_CERT: &str = "/etc/nebula/host.crt";
const NEBULA_CERT_BINARY: &str = "/usr/bin/nebula-cert";
const MACHINE_ID: &str = "/etc/machine-id";
const BOOT_ID: &str = "/proc/sys/kernel/random/boot_id";
const CLAIM_PARSER_RUNTIME_DIR: &str = "/run/mackesd-overlay-claim";
const MAX_ACTIVE_NEBULA_CERT_BYTES: usize = 128 * 1024;
const MAX_CERT_PRINT_BYTES: usize = 64 * 1024;
const CERT_PARSER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const CERT_PARSER_POLL: std::time::Duration = std::time::Duration::from_millis(10);
const MAX_CLAIMANT_ID_BYTES: usize = 128;
const MACHINE_CLAIMANT_DOMAIN: &[u8] = b"mcnf-overlay-machine-claimant-v1";
const BOOT_CLAIMANT_DOMAIN: &[u8] = b"mcnf-overlay-boot-claimant-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverlayClaimSourceError {
    FileUnavailable(&'static str),
    UntrustedFile(&'static str),
    OversizedFile(&'static str),
    MalformedFile(&'static str),
    CertificateParserUnavailable,
    CertificateParserTimedOut,
    CertificateParserOutputTooLarge,
    MalformedCertificateFacts,
    NodeMismatch,
    AddressMismatch,
    ClaimRejected,
}

impl std::fmt::Display for OverlayClaimSourceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FileUnavailable(kind) => write!(formatter, "{kind}-unavailable"),
            Self::UntrustedFile(kind) => write!(formatter, "{kind}-untrusted"),
            Self::OversizedFile(kind) => write!(formatter, "{kind}-oversized"),
            Self::MalformedFile(kind) => write!(formatter, "{kind}-malformed"),
            Self::CertificateParserUnavailable => {
                formatter.write_str("nebula-certificate-parser-unavailable")
            }
            Self::CertificateParserTimedOut => {
                formatter.write_str("nebula-certificate-parser-timed-out")
            }
            Self::CertificateParserOutputTooLarge => {
                formatter.write_str("nebula-certificate-parser-output-oversized")
            }
            Self::MalformedCertificateFacts => {
                formatter.write_str("nebula-certificate-facts-malformed")
            }
            Self::NodeMismatch => formatter.write_str("nebula-certificate-node-mismatch"),
            Self::AddressMismatch => formatter.write_str("nebula-certificate-address-mismatch"),
            Self::ClaimRejected => formatter.write_str("overlay-identity-claim-rejected"),
        }
    }
}

#[derive(Debug, Clone)]
struct OverlayClaimSourcePaths {
    certificate: PathBuf,
    machine_id: PathBuf,
    boot_id: PathBuf,
    parser_runtime_dir: PathBuf,
    trusted_uid: u32,
}

fn select_active_nebula_certificate(
    current_switch: &Path,
    active_certificate: PathBuf,
    legacy_certificate: PathBuf,
) -> PathBuf {
    match std::fs::symlink_metadata(current_switch) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => legacy_certificate,
        _ => active_certificate,
    }
}

impl OverlayClaimSourcePaths {
    fn production() -> Self {
        let active_certificate = PathBuf::from(ACTIVE_NEBULA_CERT);
        // The generation-backed identity is authoritative whenever its
        // `current` switch exists or cannot be inspected. A broken, unsafe, or
        // unreadable switch must fail in `read_no_follow`; it must never be
        // hidden by the legacy flat certificate. The flat layout is admitted
        // only when the switch is genuinely absent.
        let certificate = select_active_nebula_certificate(
            Path::new(ACTIVE_NEBULA_CURRENT),
            active_certificate,
            PathBuf::from(LEGACY_NEBULA_CERT),
        );
        Self {
            certificate,
            machine_id: PathBuf::from(MACHINE_ID),
            boot_id: PathBuf::from(BOOT_ID),
            parser_runtime_dir: PathBuf::from(CLAIM_PARSER_RUNTIME_DIR),
            trusted_uid: 0,
        }
    }
}

/// One heartbeat row, as written by a peer's `mackesd` into
/// `<peer>/mackesd/heartbeat.json` and ingested by the leader.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Heartbeat {
    /// Stable node id (matches `nodes.id` on the leader's side).
    pub node_id: String,
    /// Unix epoch milliseconds the peer's `mackesd` recorded this row.
    pub at_ms: i64,
    /// Agent version (Cargo package version of the writing `mackesd`).
    pub agent_version: String,
    /// Most recent applied revision id this peer has reconciled to,
    /// or `None` if no revision has applied yet.
    pub applied_revision: Option<String>,
    /// One of `healthy` / `degraded` / `unreachable`, per the
    /// 12.3.3 threshold table.
    pub health: HealthState,
}

/// Health-state tri-state. Stored as snake_case strings in JSON to
/// match the column the SQL store uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    /// Heartbeat lag under one cycle (10 s).
    Healthy,
    /// Heartbeat missed exactly one cycle.
    Degraded,
    /// Heartbeat missed 3+ cycles.
    Unreachable,
}

/// One link-telemetry row covering one peer's view of one other peer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkSample {
    /// The peer this sample was measured FROM.
    pub from_id: String,
    /// The peer it was measured TO.
    pub to_id: String,
    /// Round-trip time in milliseconds (median over the measurement
    /// window). `None` when the probe couldn't reach.
    pub rtt_ms: Option<u32>,
    /// Packet loss fraction `0.0..=1.0`. `None` when unmeasured.
    pub loss: Option<f32>,
    /// Throughput in Mbps. `None` when unmeasured.
    pub throughput_mbps: Option<f32>,
    /// Unix epoch milliseconds the row was sampled.
    pub at_ms: i64,
}

/// Compute the right `HealthState` for a given heartbeat-age in
/// milliseconds. Per 12.3.3: 1 missed cycle (10–20 s) = degraded;
/// 3+ missed (≥ 30 s) = unreachable.
#[must_use]
pub const fn health_state_from_age(age_ms: u64) -> HealthState {
    if age_ms >= 30_000 {
        HealthState::Unreachable
    } else if age_ms >= 10_000 {
        HealthState::Degraded
    } else {
        HealthState::Healthy
    }
}

/// Build the on-disk path a peer's heartbeat JSON lives at.
#[must_use]
pub fn heartbeat_path(workgroup_root: &Path, node_id: &str) -> PathBuf {
    workgroup_root
        .join(node_id)
        .join("mackesd")
        .join("heartbeat.json")
}

/// Build the on-disk path a peer's link-sample JSON lives at.
#[must_use]
pub fn links_path(workgroup_root: &Path, node_id: &str) -> PathBuf {
    workgroup_root
        .join(node_id)
        .join("mackesd")
        .join("links.json")
}

/// 12.3.3 heartbeat cadence. Locked at 10 s per the lock.
pub const HEARTBEAT_INTERVAL_S: u64 = 10;

fn read_trusted_claimant_file(
    path: &Path,
    max_bytes: usize,
    trusted_uid: u32,
    kind: &'static str,
) -> Result<Vec<u8>, OverlayClaimSourceError> {
    use std::io::Read as _;
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let mut current = PathBuf::new();
    for component in path.parent().into_iter().flat_map(Path::components) {
        current.push(component);
        if current.as_os_str().is_empty() {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&current)
            .map_err(|_| OverlayClaimSourceError::FileUnavailable(kind))?;
        if metadata.file_type().is_symlink() {
            return Err(OverlayClaimSourceError::UntrustedFile(kind));
        }
    }

    let leaf = std::fs::symlink_metadata(path)
        .map_err(|_| OverlayClaimSourceError::FileUnavailable(kind))?;
    if leaf.file_type().is_symlink() {
        return Err(OverlayClaimSourceError::UntrustedFile(kind));
    }

    let file: std::fs::File = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|_| OverlayClaimSourceError::FileUnavailable(kind))?
    .into();
    let metadata = file
        .metadata()
        .map_err(|_| OverlayClaimSourceError::FileUnavailable(kind))?;
    if !metadata.file_type().is_file()
        || metadata.uid() != trusted_uid
        || metadata.permissions().mode() & 0o022 != 0
    {
        return Err(OverlayClaimSourceError::UntrustedFile(kind));
    }
    // procfs pseudo-files (notably `/proc/sys/kernel/random/boot_id`) report
    // an st_size of zero even though a bounded read returns their content.
    // Treat only an oversized advertised length as an early refusal; the
    // bounded read below remains authoritative for both regular files and
    // pseudo-files, and still rejects an actually empty result.
    if metadata.len() > max_bytes as u64 {
        return Err(OverlayClaimSourceError::OversizedFile(kind));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((max_bytes + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| OverlayClaimSourceError::FileUnavailable(kind))?;
    if bytes.is_empty() || bytes.len() > max_bytes {
        return Err(OverlayClaimSourceError::OversizedFile(kind));
    }
    Ok(bytes)
}

fn read_active_nebula_certificate(
    path: &Path,
    trusted_uid: u32,
) -> Result<Vec<u8>, OverlayClaimSourceError> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let is_identity_current_layout = path
        .parent()
        .is_some_and(|parent| parent.file_name() == Some(std::ffi::OsStr::new("current")))
        && path
            .parent()
            .and_then(Path::parent)
            .is_some_and(|root| root.file_name() == Some(std::ffi::OsStr::new("identity")));
    if is_identity_current_layout
        && !path
            .parent()
            .and_then(|current| std::fs::symlink_metadata(current).ok())
            .is_some_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(OverlayClaimSourceError::UntrustedFile("nebula-certificate"));
    }

    // `read_no_follow` is the repository authority for both admitted layouts:
    // a flat regular `host.crt`, or identity/current/host.crt where `current`
    // is a single safe relative generation link beneath an owner-controlled
    // mode-0700 identity root. It validates that link, generation ownership and
    // mode, then opens the generation leaf with O_NOFOLLOW.
    let bytes = crate::ca::seal::read_no_follow(path)
        .map_err(|_| OverlayClaimSourceError::UntrustedFile("nebula-certificate"))?;
    let admitted = std::fs::metadata(path)
        .map_err(|_| OverlayClaimSourceError::FileUnavailable("nebula-certificate"))?;
    if !admitted.file_type().is_file()
        || admitted.uid() != trusted_uid
        || admitted.permissions().mode() & 0o022 != 0
    {
        return Err(OverlayClaimSourceError::UntrustedFile("nebula-certificate"));
    }
    if bytes.is_empty()
        || bytes.len() > MAX_ACTIVE_NEBULA_CERT_BYTES
        || admitted.len() != bytes.len() as u64
    {
        return Err(OverlayClaimSourceError::OversizedFile("nebula-certificate"));
    }
    Ok(bytes)
}

fn parse_machine_id(bytes: &[u8]) -> Result<String, OverlayClaimSourceError> {
    parse_single_line(bytes, "machine-id").and_then(|value| {
        if value.len() == 32
            && !value.bytes().all(|byte| byte == b'0')
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            Ok(value)
        } else {
            Err(OverlayClaimSourceError::MalformedFile("machine-id"))
        }
    })
}

fn parse_boot_id(bytes: &[u8]) -> Result<String, OverlayClaimSourceError> {
    parse_single_line(bytes, "boot-id").and_then(|value| {
        let valid = value.len() == 36
            && value.bytes().enumerate().all(|(index, byte)| match index {
                8 | 13 | 18 | 23 => byte == b'-',
                _ => byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'),
            })
            && !value
                .bytes()
                .filter(|byte| *byte != b'-')
                .all(|byte| byte == b'0');
        if valid {
            Ok(value)
        } else {
            Err(OverlayClaimSourceError::MalformedFile("boot-id"))
        }
    })
}

fn parse_single_line(bytes: &[u8], kind: &'static str) -> Result<String, OverlayClaimSourceError> {
    if bytes.len() > MAX_CLAIMANT_ID_BYTES {
        return Err(OverlayClaimSourceError::OversizedFile(kind));
    }
    let text =
        std::str::from_utf8(bytes).map_err(|_| OverlayClaimSourceError::MalformedFile(kind))?;
    let value = text.strip_suffix('\n').unwrap_or(text);
    if value.is_empty() || value.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return Err(OverlayClaimSourceError::MalformedFile(kind));
    }
    Ok(value.to_string())
}

fn claimant_digest(domain: &[u8], certificate_fingerprint: &str, value: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update([0]);
    digest.update(certificate_fingerprint.as_bytes());
    digest.update([0]);
    digest.update(value.as_bytes());
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

struct TemporaryCertificate {
    path: PathBuf,
}

impl Drop for TemporaryCertificate {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn open_trusted_runtime_directory(
    path: &Path,
    trusted_uid: u32,
) -> Result<std::fs::File, OverlayClaimSourceError> {
    use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};

    let mut current = PathBuf::new();
    for component in path.parent().into_iter().flat_map(Path::components) {
        current.push(component);
        if current.as_os_str().is_empty() {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&current)
            .map_err(|_| OverlayClaimSourceError::FileUnavailable("parser-runtime"))?;
        if metadata.file_type().is_symlink() {
            return Err(OverlayClaimSourceError::UntrustedFile("parser-runtime"));
        }
    }
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(OverlayClaimSourceError::UntrustedFile("parser-runtime"));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700);
            builder
                .create(path)
                .map_err(|_| OverlayClaimSourceError::FileUnavailable("parser-runtime"))?;
        }
        Err(_) => return Err(OverlayClaimSourceError::FileUnavailable("parser-runtime")),
    }
    let directory: std::fs::File = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|_| OverlayClaimSourceError::UntrustedFile("parser-runtime"))?
    .into();
    let metadata = directory
        .metadata()
        .map_err(|_| OverlayClaimSourceError::FileUnavailable("parser-runtime"))?;
    if !metadata.is_dir()
        || metadata.uid() != trusted_uid
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(OverlayClaimSourceError::UntrustedFile("parser-runtime"));
    }
    Ok(directory)
}

fn print_active_nebula_certificate_in(
    certificate: &[u8],
    runtime_dir: &Path,
    trusted_uid: u32,
    run_parser: impl FnOnce(&Path) -> Result<String, OverlayClaimSourceError>,
) -> Result<String, OverlayClaimSourceError> {
    use rand::RngCore as _;
    use std::io::Write as _;
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    if certificate.is_empty() || certificate.len() > MAX_ACTIVE_NEBULA_CERT_BYTES {
        return Err(OverlayClaimSourceError::OversizedFile("nebula-certificate"));
    }
    let directory = open_trusted_runtime_directory(runtime_dir, trusted_uid)?;
    let mut random = [0_u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut random);
    let file_name = format!(
        ".nebula-claim-{}.crt",
        random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let stage_path = runtime_dir.join(&file_name);
    let stage = rustix::fs::openat(
        &directory,
        file_name.as_str(),
        rustix::fs::OFlags::WRONLY
            | rustix::fs::OFlags::CREATE
            | rustix::fs::OFlags::EXCL
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .map_err(|_| OverlayClaimSourceError::FileUnavailable("parser-stage"))?;
    let cleanup = TemporaryCertificate {
        path: stage_path.clone(),
    };
    let mut stage_file: std::fs::File = stage.into();
    stage_file
        .write_all(certificate)
        .and_then(|()| stage_file.sync_all())
        .map_err(|_| OverlayClaimSourceError::FileUnavailable("parser-stage"))?;
    let metadata = stage_file
        .metadata()
        .map_err(|_| OverlayClaimSourceError::FileUnavailable("parser-stage"))?;
    if !metadata.file_type().is_file()
        || metadata.uid() != trusted_uid
        || metadata.permissions().mode() & 0o777 != 0o600
        || metadata.len() != certificate.len() as u64
    {
        return Err(OverlayClaimSourceError::UntrustedFile("parser-stage"));
    }
    drop(stage_file);
    let output = run_parser(&stage_path)?;
    drop(cleanup);
    if output.is_empty() || output.len() > MAX_CERT_PRINT_BYTES {
        return Err(OverlayClaimSourceError::MalformedCertificateFacts);
    }
    Ok(output)
}

#[derive(Debug)]
enum CertificateParserRead {
    Complete(Vec<u8>),
    Oversized,
    Failed,
}

fn terminate_certificate_parser(child: &mut std::process::Child) {
    if let Some(process_group) = rustix::process::Pid::from_raw(child.id() as i32) {
        let _ = rustix::process::kill_process_group(process_group, rustix::process::Signal::Kill);
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn run_nebula_certificate_parser_with(
    binary: &Path,
    path: &Path,
    timeout: std::time::Duration,
) -> Result<String, OverlayClaimSourceError> {
    use std::io::Read as _;
    use std::os::unix::process::CommandExt as _;
    use std::process::{Command, Stdio};

    if !binary.is_absolute() {
        return Err(OverlayClaimSourceError::CertificateParserUnavailable);
    }
    let mut command = Command::new(binary);
    command
        .args(["print", "-json", "-path"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // Descendants inherit the parser's process group so timeout and oversized
    // output handling can close every inherited stdout writer before return.
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|_| OverlayClaimSourceError::CertificateParserUnavailable)?;
    let Some(mut stdout) = child.stdout.take() else {
        terminate_certificate_parser(&mut child);
        return Err(OverlayClaimSourceError::CertificateParserUnavailable);
    };
    let (reader_tx, reader_rx) = std::sync::mpsc::sync_channel(1);
    if std::thread::Builder::new()
        .name("nebula-cert-claim-reader".into())
        .spawn(move || {
            let mut output = Vec::with_capacity(MAX_CERT_PRINT_BYTES.min(8 * 1024));
            let mut buffer = [0_u8; 8 * 1024];
            let result = loop {
                match stdout.read(&mut buffer) {
                    Ok(0) => break CertificateParserRead::Complete(output),
                    Ok(read) if output.len().saturating_add(read) > MAX_CERT_PRINT_BYTES => {
                        break CertificateParserRead::Oversized;
                    }
                    Ok(read) => output.extend_from_slice(&buffer[..read]),
                    Err(_) => break CertificateParserRead::Failed,
                }
            };
            let _ = reader_tx.send(result);
        })
        .is_err()
    {
        terminate_certificate_parser(&mut child);
        return Err(OverlayClaimSourceError::CertificateParserUnavailable);
    }

    let deadline = std::time::Instant::now() + timeout;
    let mut status = None;
    let mut output = None;
    loop {
        if output.is_none() {
            match reader_rx.try_recv() {
                Ok(CertificateParserRead::Complete(bytes)) => output = Some(bytes),
                Ok(CertificateParserRead::Oversized) => {
                    terminate_certificate_parser(&mut child);
                    return Err(OverlayClaimSourceError::CertificateParserOutputTooLarge);
                }
                Ok(CertificateParserRead::Failed)
                | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    terminate_certificate_parser(&mut child);
                    return Err(OverlayClaimSourceError::CertificateParserUnavailable);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(Some(exit_status)) if !exit_status.success() => {
                    terminate_certificate_parser(&mut child);
                    return Err(OverlayClaimSourceError::CertificateParserUnavailable);
                }
                Ok(Some(exit_status)) => status = Some(exit_status),
                Ok(None) => {}
                Err(_) => {
                    terminate_certificate_parser(&mut child);
                    return Err(OverlayClaimSourceError::CertificateParserUnavailable);
                }
            }
        }
        if status.is_some() && output.is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            terminate_certificate_parser(&mut child);
            return Err(OverlayClaimSourceError::CertificateParserTimedOut);
        }
        std::thread::sleep(CERT_PARSER_POLL);
    }

    let output = output.expect("parser output is present after loop");
    if output.is_empty() {
        return Err(OverlayClaimSourceError::CertificateParserUnavailable);
    }
    String::from_utf8(output).map_err(|_| OverlayClaimSourceError::MalformedCertificateFacts)
}

fn run_nebula_certificate_parser(path: &Path) -> Result<String, OverlayClaimSourceError> {
    run_nebula_certificate_parser_with(Path::new(NEBULA_CERT_BINARY), path, CERT_PARSER_TIMEOUT)
}

fn build_overlay_identity_claim_with(
    node_id: &str,
    overlay_address: &str,
    sources: &OverlayClaimSourcePaths,
    print_certificate: impl FnOnce(&[u8]) -> Result<String, OverlayClaimSourceError>,
) -> Result<mackes_mesh_types::peers::OverlayIdentityClaim, OverlayClaimSourceError> {
    let certificate = read_active_nebula_certificate(&sources.certificate, sources.trusted_uid)?;
    let printed = print_certificate(&certificate)?;
    if printed.len() > MAX_CERT_PRINT_BYTES {
        return Err(OverlayClaimSourceError::MalformedCertificateFacts);
    }
    let public = crate::ca::blocklist::parse_public_identity_json(&printed)
        .ok_or(OverlayClaimSourceError::MalformedCertificateFacts)?;
    if public.name != node_id {
        return Err(OverlayClaimSourceError::NodeMismatch);
    }
    if public.address != overlay_address {
        return Err(OverlayClaimSourceError::AddressMismatch);
    }
    let machine_id = parse_machine_id(&read_trusted_claimant_file(
        &sources.machine_id,
        MAX_CLAIMANT_ID_BYTES,
        sources.trusted_uid,
        "machine-id",
    )?)?;
    let boot_id = parse_boot_id(&read_trusted_claimant_file(
        &sources.boot_id,
        MAX_CLAIMANT_ID_BYTES,
        sources.trusted_uid,
        "boot-id",
    )?)?;
    let machine_claimant_digest =
        claimant_digest(MACHINE_CLAIMANT_DOMAIN, &public.fingerprint, &machine_id);
    let boot_claimant_digest = claimant_digest(BOOT_CLAIMANT_DOMAIN, &public.fingerprint, &boot_id);
    mackes_mesh_types::peers::OverlayIdentityClaim::new(
        node_id,
        public.name,
        public.address,
        public.fingerprint,
        machine_claimant_digest,
        boot_claimant_digest,
    )
    .map_err(|_| OverlayClaimSourceError::ClaimRejected)
}

fn build_local_overlay_identity_claim(
    node_id: &str,
    overlay_address: &str,
) -> Result<mackes_mesh_types::peers::OverlayIdentityClaim, OverlayClaimSourceError> {
    let sources = OverlayClaimSourcePaths::production();
    build_overlay_identity_claim_with(node_id, overlay_address, &sources, |certificate| {
        print_active_nebula_certificate_in(
            certificate,
            &sources.parser_runtime_dir,
            sources.trusted_uid,
            run_nebula_certificate_parser,
        )
    })
}

/// Build the canonical "this peer is healthy right now" heartbeat
/// using the current process's agent version + an `applied_revision`
/// the caller supplies. Convenience wrapper around the struct
/// literal so worker code stays one line.
#[must_use]
pub fn build_heartbeat(node_id: &str, applied_revision: Option<&str>) -> Heartbeat {
    let at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    Heartbeat {
        node_id: node_id.to_owned(),
        at_ms,
        agent_version: env!("CARGO_PKG_VERSION").to_owned(),
        applied_revision: applied_revision.map(str::to_owned),
        health: HealthState::Healthy,
    }
}

/// Spawn a background thread that writes one heartbeat every
/// `interval` until `shutdown` flips true. Returns the join handle
/// so the caller can wait on shutdown.
///
/// `interval` is the operator-tunable cadence (E1.3 #3, sourced from
/// `/etc/mackesd/mackesd.toml`); pass
/// `Duration::from_secs(HEARTBEAT_INTERVAL_S)` for the locked default.
///
/// Used by the `mackesd` reconcile loop's bootstrap to keep the
/// peer's heartbeat fresh even while the rest of the reconciler is
/// processing a long-running deploy.
pub fn spawn_heartbeat_worker(
    workgroup_root: PathBuf,
    node_id: String,
    interval: std::time::Duration,
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        use std::sync::atomic::Ordering;
        // PEERVER-2 — publish this peer's convergence record to the GFS
        // peers/ dir (read by mde-update / mde-install; mirrored into
        // nodes by PEERVER-4). Detect the mde-core RPM version once;
        // cap the write to ~once/min (§3.1 slow-state budget) rather
        // than every heartbeat. See docs/design/v2.7-peer-data-convergence.md.
        let peer_hostname = node_id
            .strip_prefix("peer:")
            .unwrap_or(&node_id)
            .to_string();
        let mde_version = detect_mde_core_version();
        let peers_dir = mackes_mesh_types::peers::peers_dir(&workgroup_root);
        // SUBSTRATE-3 — when this node is on the etcd coordination plane (the
        // endpoints file exists), the peer directory lives in etcd under a
        // keepalive lease; otherwise it's the replicated fs dir, unchanged.
        // Empty on every pre-cutover node, so the live fleet keeps the fs path.
        #[cfg(feature = "async-services")]
        let etcd_endpoints = crate::substrate::etcd::default_endpoints();
        #[cfg(not(feature = "async-services"))]
        let etcd_endpoints: Vec<String> = Vec::new();
        let peer_write_min = std::time::Duration::from_secs(60);
        let mut last_peer_write: Option<std::time::Instant> = None;
        // Check the shutdown flag every 100 ms instead of sleeping the
        // full interval between checks — otherwise a shutdown request
        // mid-interval isn't honored until the next wake (up to the
        // full HEARTBEAT_INTERVAL_S), which both stretched the
        // supervisor's SIGTERM→exit latency and raced the worker
        // shutdown test (DEAD-FLAKY-HEARTBEAT, 2026-05-28). Chunked
        // sleep makes shutdown responsive within ~100 ms.
        const CHECK_CHUNK: std::time::Duration = std::time::Duration::from_millis(100);
        while !shutdown.load(Ordering::Relaxed) {
            // SUBSTRATE-3 — the legacy fs heartbeat file is redundant once this node
            // is on the etcd plane: the peer-record (below) + the etcd keepalive lease
            // carry liveness. Writing it on an etcd node only fails noisily every
            // interval on an unmounted QNM-Shared ("would poison the mountpoint" — seen
            // in the SUBSTRATE-14 reboot drill). Skip it when etcd is active; keep it as
            // the liveness signal on every pre-cutover (fs) node, so the fallback is intact.
            if etcd_endpoints.is_empty() {
                let hb = build_heartbeat(&node_id, None);
                if let Err(e) = write_heartbeat(&workgroup_root, &hb) {
                    tracing::warn!(error = %e, "heartbeat: write failed");
                }
            }
            // PEERVER-2 — refresh the peer-convergence record at most
            // once/min (own-row authority: we are the sole writer of
            // our own <hostname>.json).
            let due = last_peer_write.map_or(true, |t| t.elapsed() >= peer_write_min);
            if due {
                // PD-2 — probe + publish the service descriptors on the
                // record-write cycle (L13: one cycle, one write); health
                // derives from the Netdata alarm tier (L15) instead of a
                // hardcoded "healthy".
                let descriptors = crate::descriptors::probe_local();
                let health = if descriptors.alarms.tier.is_empty() {
                    "healthy".to_string()
                } else {
                    descriptors.alarms.tier.clone()
                };
                let mut rec = mackes_mesh_types::peers::PeerRecord::now(
                    peer_hostname.clone(),
                    mde_version.clone(),
                    health,
                );
                rec.descriptors = Some(descriptors);
                // Record our own overlay IP into the replicated directory so
                // every node (not just the signer) can resolve <host>.mesh,
                // publish overlay services, and validate routing edges.
                rec.overlay_ip = crate::voip_rtt::own_nebula_ip();
                // LIGHTHOUSE-1/Q1 — stamp our pinned deployment role + capability
                // tags into the replicated directory so any node can identify the
                // lighthouse set (and the MEDIA-1 Lighthouse_Media subclass) from
                // the QNM-Shared peer JSON (no separate probe). `media` is the
                // §9 capability tag — orthogonal to the role, only set on a
                // media-tagged lighthouse — read off the same `role.toml`.
                if let Ok(class) = mde_role::load_class() {
                    rec.role = Some(class.role.as_str().to_string());
                    rec.media = class.is_media_lighthouse();
                }
                // LIGHTHOUSE-10 — a lighthouse stamps its PUBLIC underlay address
                // into the directory so the enroll roster can hand joining nodes
                // the FULL lighthouse set (redundancy). Only lighthouses carry it;
                // others leave it None (skipped from any built roster).
                if rec.role.as_deref() == Some(mackes_mesh_types::lighthouse::LIGHTHOUSE_ROLE) {
                    rec.external_addr = crate::lighthouse_addr::read_external_addr();
                }
                if etcd_endpoints.is_empty() {
                    match mackes_mesh_types::peers::write_peer_record(&peers_dir, &rec) {
                        Ok(_) => last_peer_write = Some(std::time::Instant::now()),
                        Err(e) => tracing::warn!(error = %e, "peer-record: write failed"),
                    }
                } else {
                    #[cfg(feature = "async-services")]
                    {
                        let claim = rec.overlay_ip.as_deref().map_or_else(
                            || Err(OverlayClaimSourceError::AddressMismatch),
                            |overlay_address| {
                                build_local_overlay_identity_claim(&node_id, overlay_address)
                            },
                        );
                        match claim {
                            Ok(claim)
                                if crate::substrate::peers::put_peer_with_overlay_identity_claim_blocking(
                                    &etcd_endpoints,
                                    &rec,
                                    &claim,
                                ) =>
                            {
                                last_peer_write = Some(std::time::Instant::now());
                                record_peer_publication_success(Path::new(
                                    "/run/mesh-health/peer-publication.ok",
                                ));
                            }
                            Ok(_) => tracing::warn!(
                                "peer-record: lease-backed peer/claim transaction failed; will retry next heartbeat"
                            ),
                            Err(error) => tracing::warn!(
                                reason = %error,
                                "peer-record: overlay claim authority unavailable; publication withheld"
                            ),
                        }
                    }
                    #[cfg(not(feature = "async-services"))]
                    {
                        tracing::warn!(
                            "peer-record: etcd peer directory requires async-services; will retry next heartbeat"
                        );
                    }
                }
            }
            // Interruptible interval sleep.
            let mut slept = std::time::Duration::ZERO;
            while slept < interval && !shutdown.load(Ordering::Relaxed) {
                std::thread::sleep(CHECK_CHUNK);
                slept += CHECK_CHUNK;
            }
        }
    })
}

fn record_peer_publication_success(path: &Path) {
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let temporary = path.with_extension("tmp");
    if std::fs::write(&temporary, b"ok\n").is_ok() {
        let _ = std::fs::rename(temporary, path);
    }
}

/// This node's installed `mde-core` RPM version (PEERVER-2), or
/// `None` when the package isn't installed / `rpm` is unavailable
/// (e.g. a dev checkout). Cheap: queried once per heartbeat-worker
/// spawn, not per tick.
#[must_use]
pub fn detect_mde_core_version() -> Option<String> {
    let out = std::process::Command::new("rpm")
        .args(["-q", "--qf", "%{VERSION}", "mde-core"])
        .output()
        .ok()?;
    if out.status.success() {
        let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!v.is_empty()).then_some(v)
    } else {
        None
    }
}

/// Atomic write of a heartbeat row to disk. Writes via a `.tmp`
/// sibling and renames into place so a reading aggregator never
/// sees a partial file.
///
/// # Errors
/// Returns `std::io::Error` when the parent directory isn't
/// writable or the rename fails.
pub fn write_heartbeat(workgroup_root: &Path, hb: &Heartbeat) -> std::io::Result<PathBuf> {
    // Guard: never write into the canonical shared dir when it doesn't exist —
    // the heartbeat would land on a bare local dir instead of the replicated one.
    if !crate::shared_root_writable(workgroup_root) {
        return Err(std::io::Error::other(format!(
            "shared dir {} is down — skipping heartbeat write (would land on a bare local dir)",
            workgroup_root.display()
        )));
    }
    let path = heartbeat_path(workgroup_root, &hb.node_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(hb)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Atomic write of a link-sample batch to disk. Same pattern.
///
/// # Errors
/// Returns `std::io::Error` when the parent directory isn't
/// writable or the rename fails.
pub fn write_links(
    workgroup_root: &Path,
    node_id: &str,
    samples: &[LinkSample],
) -> std::io::Result<PathBuf> {
    let path = links_path(workgroup_root, node_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(samples)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CERT_FINGERPRINT: &str =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const ALTERNATE_CERT_FINGERPRINT: &str =
        "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
    const MACHINE_A: &str = "13579bdf2468ace013579bdf2468ace0";
    const MACHINE_B: &str = "02468ace13579bdf02468ace13579bdf";
    const BOOT_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const BOOT_B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    const CERT_BYTES: &[u8] = b"public-nebula-certificate-fixture";

    fn claim_sources(root: &Path, machine_id: &str, boot_id: &str) -> OverlayClaimSourcePaths {
        use std::os::unix::fs::MetadataExt as _;

        let certificate = root.join("host.crt");
        let machine = root.join("machine-id");
        let boot = root.join("boot-id");
        std::fs::write(&certificate, CERT_BYTES).expect("certificate fixture");
        std::fs::write(&machine, format!("{machine_id}\n")).expect("machine fixture");
        std::fs::write(&boot, format!("{boot_id}\n")).expect("boot fixture");
        let trusted_uid = std::fs::metadata(&certificate)
            .expect("certificate metadata")
            .uid();
        OverlayClaimSourcePaths {
            certificate,
            machine_id: machine,
            boot_id: boot,
            parser_runtime_dir: root.join("parser-runtime"),
            trusted_uid,
        }
    }

    fn certificate_json(node_id: &str, address: &str) -> String {
        format!(
            r#"{{"details":{{"name":"{node_id}","ips":["{address}/17"]}},"fingerprint":"{CERT_FINGERPRINT}"}}"#
        )
    }

    fn build_fixture_claim(
        sources: &OverlayClaimSourcePaths,
    ) -> Result<mackes_mesh_types::peers::OverlayIdentityClaim, OverlayClaimSourceError> {
        build_overlay_identity_claim_with("peer:SURFACE", "10.42.0.7", sources, |certificate| {
            assert_eq!(certificate, CERT_BYTES);
            Ok(certificate_json("peer:SURFACE", "10.42.0.7"))
        })
    }

    #[test]
    fn overlay_claim_distinguishes_current_boots_deterministically() {
        let temp = tempfile::tempdir().expect("tempdir");
        let sources = claim_sources(temp.path(), MACHINE_A, BOOT_A);
        let first = build_fixture_claim(&sources).expect("first boot");
        std::fs::write(&sources.boot_id, format!("{BOOT_B}\n")).expect("second boot");
        let second = build_fixture_claim(&sources).expect("second boot");

        assert_eq!(
            first.machine_claimant_digest,
            second.machine_claimant_digest
        );
        assert_ne!(first.boot_claimant_digest, second.boot_claimant_digest);
        assert_eq!(
            second,
            build_fixture_claim(&claim_sources(temp.path(), MACHINE_A, BOOT_B)).expect("repeat")
        );
    }

    #[test]
    fn overlay_claim_distinguishes_copied_identity_on_distinct_machines() {
        let first_root = tempfile::tempdir().expect("first tempdir");
        let second_root = tempfile::tempdir().expect("second tempdir");
        let first = build_fixture_claim(&claim_sources(first_root.path(), MACHINE_A, BOOT_A))
            .expect("first machine");
        let second = build_fixture_claim(&claim_sources(second_root.path(), MACHINE_B, BOOT_A))
            .expect("second machine");

        assert_ne!(
            first.machine_claimant_digest,
            second.machine_claimant_digest
        );
        assert_eq!(first.boot_claimant_digest, second.boot_claimant_digest);
        assert_eq!(
            first.certificate_fingerprint,
            second.certificate_fingerprint
        );
        assert_eq!(first.nebula_address, second.nebula_address);
    }

    #[test]
    fn overlay_claimant_digests_are_scoped_to_certificate_identity() {
        let machine_under_first =
            claimant_digest(MACHINE_CLAIMANT_DOMAIN, CERT_FINGERPRINT, MACHINE_A);
        let machine_under_reenrollment = claimant_digest(
            MACHINE_CLAIMANT_DOMAIN,
            ALTERNATE_CERT_FINGERPRINT,
            MACHINE_A,
        );
        let boot_under_first = claimant_digest(BOOT_CLAIMANT_DOMAIN, CERT_FINGERPRINT, BOOT_A);
        let boot_under_reenrollment =
            claimant_digest(BOOT_CLAIMANT_DOMAIN, ALTERNATE_CERT_FINGERPRINT, BOOT_A);

        assert_ne!(machine_under_first, machine_under_reenrollment);
        assert_ne!(boot_under_first, boot_under_reenrollment);
        assert_eq!(
            machine_under_first,
            claimant_digest(MACHINE_CLAIMANT_DOMAIN, CERT_FINGERPRINT, MACHINE_A)
        );
        assert!(!machine_under_first.contains(MACHINE_A));
        assert!(!boot_under_first.contains(BOOT_A));
    }

    #[test]
    fn overlay_claim_rejects_malformed_oversized_and_symlinked_sources() {
        let temp = tempfile::tempdir().expect("tempdir");
        let sources = claim_sources(temp.path(), MACHINE_A, BOOT_A);

        std::fs::write(&sources.machine_id, "not-a-machine-id\n").expect("malformed machine");
        assert_eq!(
            build_fixture_claim(&sources),
            Err(OverlayClaimSourceError::MalformedFile("machine-id"))
        );

        std::fs::write(&sources.machine_id, format!("{MACHINE_A}\n")).expect("restore machine");
        std::fs::write(&sources.boot_id, vec![b'a'; MAX_CLAIMANT_ID_BYTES + 1])
            .expect("oversized boot");
        assert_eq!(
            build_fixture_claim(&sources),
            Err(OverlayClaimSourceError::OversizedFile("boot-id"))
        );

        std::fs::write(&sources.boot_id, format!("{BOOT_A}\n")).expect("restore boot");
        let machine_target = temp.path().join("machine-target");
        std::fs::write(&machine_target, format!("{MACHINE_A}\n")).expect("machine target");
        std::fs::remove_file(&sources.machine_id).expect("remove machine fixture");
        std::os::unix::fs::symlink(&machine_target, &sources.machine_id).expect("machine symlink");
        assert_eq!(
            build_fixture_claim(&sources),
            Err(OverlayClaimSourceError::UntrustedFile("machine-id"))
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn procfs_boot_id_zero_stat_size_is_read_from_content() {
        let path = Path::new("/proc/sys/kernel/random/boot_id");
        let metadata = std::fs::metadata(path).expect("boot id metadata");
        if metadata.len() != 0 {
            // The zero-sized stat contract is Linux procfs behavior; keep the
            // test portable across unusual test sandboxes while still testing
            // the production path whenever procfs exposes that contract.
            return;
        }
        let bytes = read_trusted_claimant_file(path, MAX_CLAIMANT_ID_BYTES, 0, "boot-id")
            .expect("procfs boot id content should be readable");
        assert!(parse_boot_id(&bytes).is_ok());
    }

    #[test]
    fn active_certificate_accepts_only_safe_generation_switch_or_flat_layout() {
        use std::os::unix::fs::{symlink, DirBuilderExt as _, PermissionsExt as _};

        let temp = tempfile::tempdir().expect("tempdir");
        let mut sources = claim_sources(temp.path(), MACHINE_A, BOOT_A);
        let identity = temp.path().join("identity");
        let generation = identity.join("generation-test");
        let mut identity_builder = std::fs::DirBuilder::new();
        identity_builder
            .mode(0o700)
            .create(&identity)
            .expect("identity");
        let mut generation_builder = std::fs::DirBuilder::new();
        generation_builder
            .mode(0o700)
            .create(&generation)
            .expect("generation");
        std::fs::write(generation.join("host.crt"), CERT_BYTES).expect("generation cert");
        symlink("generation-test", identity.join("current")).expect("current switch");
        sources.certificate = identity.join("current/host.crt");

        assert!(build_fixture_claim(&sources).is_ok());

        std::fs::set_permissions(&generation, std::fs::Permissions::from_mode(0o755))
            .expect("weaken generation mode");
        assert_eq!(
            build_fixture_claim(&sources),
            Err(OverlayClaimSourceError::UntrustedFile("nebula-certificate"))
        );
        std::fs::set_permissions(&generation, std::fs::Permissions::from_mode(0o700))
            .expect("restore generation mode");

        std::fs::remove_file(identity.join("current")).expect("remove current");
        symlink("../escape", identity.join("current")).expect("unsafe current");
        assert_eq!(
            build_fixture_claim(&sources),
            Err(OverlayClaimSourceError::UntrustedFile("nebula-certificate"))
        );

        std::fs::remove_file(identity.join("current")).expect("remove unsafe current");
        std::fs::create_dir(identity.join("current")).expect("non-link current directory");
        std::fs::write(identity.join("current/host.crt"), CERT_BYTES)
            .expect("non-link current cert");
        assert_eq!(
            build_fixture_claim(&sources),
            Err(OverlayClaimSourceError::UntrustedFile("nebula-certificate"))
        );
    }

    #[test]
    fn invalid_current_switch_cannot_be_hidden_by_flat_legacy_layout() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let current = temp.path().join("current");
        let active = temp.path().join("identity/current/host.crt");
        let legacy = temp.path().join("host.crt");

        assert_eq!(
            select_active_nebula_certificate(&current, active.clone(), legacy.clone()),
            legacy
        );
        symlink("../escape", &current).expect("unsafe current switch");
        assert_eq!(
            select_active_nebula_certificate(&current, active.clone(), legacy),
            active
        );
    }

    #[test]
    fn certificate_parser_consumes_admitted_snapshot_and_cleans_stage() {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

        let temp = tempfile::tempdir().expect("tempdir");
        let live = temp.path().join("host.crt");
        std::fs::write(&live, CERT_BYTES).expect("live cert");
        let trusted_uid = std::fs::metadata(&live).expect("live metadata").uid();
        let admitted = read_active_nebula_certificate(&live, trusted_uid).expect("admit cert");
        std::fs::write(&live, b"attacker-swapped-live-certificate").expect("swap live cert");
        let runtime = temp.path().join("runtime");
        let mut staged_path = None;

        let printed =
            print_active_nebula_certificate_in(&admitted, &runtime, trusted_uid, |path| {
                let staged = std::fs::read(path).expect("read staged snapshot");
                let metadata = std::fs::metadata(path).expect("stage metadata");
                assert_eq!(staged, CERT_BYTES);
                assert_ne!(staged, std::fs::read(&live).expect("read swapped live"));
                assert_eq!(metadata.uid(), trusted_uid);
                assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
                staged_path = Some(path.to_path_buf());
                Ok(certificate_json("peer:SURFACE", "10.42.0.7"))
            })
            .expect("parse admitted snapshot");

        assert!(printed.contains(CERT_FINGERPRINT));
        assert!(!staged_path.expect("stage path captured").exists());

        let mut failed_stage_path = None;
        let failed = print_active_nebula_certificate_in(&admitted, &runtime, trusted_uid, |path| {
            failed_stage_path = Some(path.to_path_buf());
            Err(OverlayClaimSourceError::CertificateParserUnavailable)
        });
        assert_eq!(
            failed,
            Err(OverlayClaimSourceError::CertificateParserUnavailable)
        );
        assert!(!failed_stage_path.expect("failed stage captured").exists());
    }

    fn executable_parser_fixture(root: &Path, name: &str, script: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;

        let path = root.join(name);
        std::fs::write(&path, script).expect("write parser fixture");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .expect("make parser fixture executable");
        path
    }

    #[test]
    fn certificate_parser_kills_oversized_and_hung_process_groups() {
        let temp = tempfile::tempdir().expect("tempdir");
        let certificate = temp.path().join("admitted.crt");
        std::fs::write(&certificate, CERT_BYTES).expect("certificate fixture");
        let oversized = executable_parser_fixture(
            temp.path(),
            "oversized-parser",
            "#!/bin/sh\nwhile :; do printf '0123456789abcdef0123456789abcdef'; done\n",
        );
        let hung = executable_parser_fixture(
            temp.path(),
            "hung-parser",
            "#!/bin/sh\nwhile :; do :; done\n",
        );

        let oversized_started = std::time::Instant::now();
        assert_eq!(
            run_nebula_certificate_parser_with(
                &oversized,
                &certificate,
                std::time::Duration::from_secs(1),
            ),
            Err(OverlayClaimSourceError::CertificateParserOutputTooLarge)
        );
        assert!(oversized_started.elapsed() < std::time::Duration::from_secs(2));

        let hung_started = std::time::Instant::now();
        assert_eq!(
            run_nebula_certificate_parser_with(
                &hung,
                &certificate,
                std::time::Duration::from_millis(100),
            ),
            Err(OverlayClaimSourceError::CertificateParserTimedOut)
        );
        assert!(hung_started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn overlay_claim_rejects_certificate_node_address_and_parser_mismatch() {
        let temp = tempfile::tempdir().expect("tempdir");
        let sources = claim_sources(temp.path(), MACHINE_A, BOOT_A);

        let wrong_node =
            build_overlay_identity_claim_with("peer:SURFACE", "10.42.0.7", &sources, |_| {
                Ok(certificate_json("peer:OTHER", "10.42.0.7"))
            });
        assert_eq!(wrong_node, Err(OverlayClaimSourceError::NodeMismatch));
        let wrong_address =
            build_overlay_identity_claim_with("peer:SURFACE", "10.42.0.7", &sources, |_| {
                Ok(certificate_json("peer:SURFACE", "10.42.0.8"))
            });
        assert_eq!(wrong_address, Err(OverlayClaimSourceError::AddressMismatch));
        let malformed =
            build_overlay_identity_claim_with("peer:SURFACE", "10.42.0.7", &sources, |_| {
                Ok("{not-json".into())
            });
        assert_eq!(
            malformed,
            Err(OverlayClaimSourceError::MalformedCertificateFacts)
        );
    }

    #[test]
    fn overlay_claim_wire_contains_no_raw_ids_paths_or_certificate_material() {
        let temp = tempfile::tempdir().expect("tempdir");
        let sources = claim_sources(temp.path(), MACHINE_A, BOOT_A);
        let claim = build_fixture_claim(&sources).expect("claim");
        let wire = claim.to_json().expect("wire");

        assert!(!wire.contains(MACHINE_A));
        assert!(!wire.contains(BOOT_A));
        assert!(!wire.contains(std::str::from_utf8(CERT_BYTES).expect("fixture utf8")));
        assert!(!wire.contains(&sources.machine_id.display().to_string()));
        assert!(!wire.contains(&sources.boot_id.display().to_string()));
        assert!(!wire.contains(&sources.certificate.display().to_string()));
        assert!(claim.validate().is_ok());
    }

    #[test]
    fn health_state_thresholds_match_lock() {
        assert_eq!(health_state_from_age(0), HealthState::Healthy);
        assert_eq!(health_state_from_age(5_000), HealthState::Healthy);
        assert_eq!(health_state_from_age(10_000), HealthState::Degraded);
        assert_eq!(health_state_from_age(20_000), HealthState::Degraded);
        assert_eq!(health_state_from_age(30_000), HealthState::Unreachable);
        assert_eq!(health_state_from_age(120_000), HealthState::Unreachable);
    }

    #[test]
    fn heartbeat_path_shape() {
        let p = heartbeat_path(Path::new("/tmp/qnm"), "peer:anvil");
        assert!(p.ends_with("peer:anvil/mackesd/heartbeat.json"));
    }

    #[test]
    fn successful_peer_publication_stamp_is_atomic_and_refreshable() {
        let dir = tempfile::tempdir().unwrap();
        let stamp = dir.path().join("peer-publication.ok");
        record_peer_publication_success(&stamp);
        assert_eq!(std::fs::read(&stamp).unwrap(), b"ok\n");
        assert!(!stamp.with_extension("tmp").exists());
        record_peer_publication_success(&stamp);
        assert_eq!(std::fs::read(&stamp).unwrap(), b"ok\n");
    }

    #[test]
    fn write_heartbeat_round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let hb = Heartbeat {
            node_id: "peer:a".into(),
            at_ms: 1_234_567,
            agent_version: "1.1.0".into(),
            applied_revision: Some("r-2026-05-19-0001".into()),
            health: HealthState::Healthy,
        };
        let path = write_heartbeat(dir.path(), &hb).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let back: Heartbeat = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back, hb);
    }

    #[test]
    fn write_links_round_trips_a_batch() {
        let dir = tempfile::tempdir().unwrap();
        let samples = vec![
            LinkSample {
                from_id: "peer:a".into(),
                to_id: "peer:b".into(),
                rtt_ms: Some(12),
                loss: Some(0.0),
                throughput_mbps: Some(950.0),
                at_ms: 1,
            },
            LinkSample {
                from_id: "peer:a".into(),
                to_id: "peer:c".into(),
                rtt_ms: None,
                loss: None,
                throughput_mbps: None,
                at_ms: 1,
            },
        ];
        let path = write_links(dir.path(), "peer:a", &samples).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let back: Vec<LinkSample> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].rtt_ms, Some(12));
        assert!(back[1].rtt_ms.is_none());
    }

    #[test]
    fn json_round_trips_through_serde() {
        let hb = Heartbeat {
            node_id: "peer:x".into(),
            at_ms: 0,
            agent_version: "1.1.0".into(),
            applied_revision: None,
            health: HealthState::Unreachable,
        };
        let json = serde_json::to_string(&hb).unwrap();
        let back: Heartbeat = serde_json::from_str(&json).unwrap();
        assert_eq!(back, hb);
    }

    #[test]
    fn build_heartbeat_uses_cargo_version() {
        let hb = build_heartbeat("peer:test", None);
        assert_eq!(hb.agent_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(hb.node_id, "peer:test");
        assert_eq!(hb.health, HealthState::Healthy);
        assert!(hb.applied_revision.is_none());
    }

    #[test]
    fn build_heartbeat_carries_applied_revision_when_set() {
        let hb = build_heartbeat("peer:test", Some("r-2026-05-19-0042"));
        assert_eq!(hb.applied_revision.as_deref(), Some("r-2026-05-19-0042"));
    }

    #[test]
    fn heartbeat_worker_exits_on_shutdown_flag() {
        let dir = tempfile::tempdir().unwrap();
        let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let h = spawn_heartbeat_worker(
            dir.path().to_path_buf(),
            "peer:test".into(),
            std::time::Duration::from_secs(HEARTBEAT_INTERVAL_S),
            std::sync::Arc::clone(&shutdown),
        );
        // The worker writes the heartbeat at the start of its first loop
        // iteration. POLL for it rather than assuming a fixed 100 ms tick —
        // under heavy parallel test load the worker thread can take far longer
        // than 100 ms to be scheduled, which made the old fixed-sleep assertion
        // flaky (it raced the very first write). Bound the wait to 5 s.
        let path = heartbeat_path(dir.path(), "peer:test");
        let step = std::time::Duration::from_millis(20);
        let mut waited = std::time::Duration::ZERO;
        while !path.exists() && waited < std::time::Duration::from_secs(5) {
            std::thread::sleep(step);
            waited += step;
        }
        assert!(
            path.exists(),
            "expected {path:?} to exist after one tick (waited {waited:?})"
        );
        // Flip shutdown; the chunked-sleep loop honors it within ~100 ms, so
        // join() returns promptly (no full-interval wait).
        shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = h.join();
    }
}
