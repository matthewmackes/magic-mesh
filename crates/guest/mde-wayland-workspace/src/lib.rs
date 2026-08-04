//! Guest-only lifecycle and confinement for one Wayland application workspace.
//!
//! Construct's host deliberately has no compositor. This crate runs only in an
//! App VM: it starts a fixed guest compositor, waits for its private Wayland
//! socket, launches one admitted Flatpak identity, and owns both processes until
//! the session ends. Flatpak is the application sandbox; the VM is the hard host
//! security boundary.

use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::flag;

/// Fixed 1080p App VM output width.
pub const OUTPUT_WIDTH: u16 = 1920;
/// Fixed 1080p App VM output height.
pub const OUTPUT_HEIGHT: u16 = 1080;
const START_TIMEOUT: Duration = Duration::from_secs(20);

/// Files and binaries owned by the immutable App VM image.
#[derive(Clone, Debug)]
pub struct WorkspacePaths {
    /// Root containing the admitted app and session identities.
    pub input_root: PathBuf,
    /// Guest hostname file, used as the VM identity.
    pub hostname: PathBuf,
    /// Writable runtime root for private sockets and compositor config.
    pub runtime_root: PathBuf,
    /// Monotonic runtime-evidence generation.
    pub generation: PathBuf,
    /// Image-owned admission validator.
    pub validator: PathBuf,
    /// D-Bus session wrapper.
    pub dbus_run_session: PathBuf,
    /// Guest compositor binary.
    pub compositor: PathBuf,
    /// Curated Flatpak client.
    pub flatpak: PathBuf,
    /// Compositor control client.
    pub swaymsg: PathBuf,
    /// Mesh Bus publisher.
    pub bus: PathBuf,
}

impl Default for WorkspacePaths {
    fn default() -> Self {
        Self {
            input_root: PathBuf::from("/etc/mackesd/app-vm"),
            hostname: PathBuf::from("/etc/hostname"),
            runtime_root: PathBuf::from("/run/mcnf-wayland-workspace"),
            generation: PathBuf::from("/var/lib/mackesd/app-vm/generation"),
            validator: PathBuf::from("/usr/local/libexec/mcnf-app-vm-validate"),
            dbus_run_session: PathBuf::from("/usr/bin/dbus-run-session"),
            compositor: PathBuf::from("/usr/bin/sway"),
            flatpak: PathBuf::from("/usr/bin/flatpak"),
            swaymsg: PathBuf::from("/usr/bin/swaymsg"),
            bus: PathBuf::from("/usr/bin/mde-bus"),
        }
    }
}

/// Validated, command-free identity inputs for one workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceIdentity {
    /// Curated reverse-DNS Flatpak application ID.
    pub app_id: String,
    /// Broker-issued session identity.
    pub session_id: String,
    /// App VM hostname.
    pub vm_id: String,
}

/// Runtime errors surfaced by the guest workspace supervisor.
#[derive(Debug)]
pub enum WorkspaceError {
    /// A required image-owned file or process operation failed.
    Io { operation: &'static str, source: io::Error },
    /// An identity did not match the bounded guest contract.
    InvalidIdentity(&'static str),
    /// The image-owned validator rejected runtime inputs.
    AdmissionRejected(ExitStatus),
    /// The D-Bus session wrapper returned unsuccessfully.
    SessionFailed(ExitStatus),
    /// Sway exited before exposing its private sockets.
    CompositorExited(ExitStatus),
    /// The compositor did not expose both sockets within the bounded timeout.
    CompositorTimeout,
    /// The monotonic runtime generation is malformed or exhausted.
    BadGeneration,
}

impl core::fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io { operation, source } => write!(f, "{operation}: {source}"),
            Self::InvalidIdentity(field) => write!(f, "invalid App VM {field}"),
            Self::AdmissionRejected(status) => write!(f, "App VM admission rejected: {status}"),
            Self::SessionFailed(status) => write!(f, "Wayland workspace failed: {status}"),
            Self::CompositorExited(status) => {
                write!(f, "guest compositor exited before readiness: {status}")
            }
            Self::CompositorTimeout => f.write_str("guest compositor readiness timed out"),
            Self::BadGeneration => f.write_str("runtime evidence generation is invalid"),
        }
    }
}

impl std::error::Error for WorkspaceError {}

fn io_error(operation: &'static str, source: io::Error) -> WorkspaceError {
    WorkspaceError::Io { operation, source }
}

/// Load and independently validate the bounded App VM identity files.
pub fn load_identity(paths: &WorkspacePaths) -> Result<WorkspaceIdentity, WorkspaceError> {
    let app_id = read_identity(&paths.input_root.join("app-id"), "app identity")?;
    let session_id = read_identity(&paths.input_root.join("session-id"), "session identity")?;
    let vm_id = read_identity(&paths.hostname, "VM identity")?;
    if !valid_app_id(&app_id) {
        return Err(WorkspaceError::InvalidIdentity("app identity"));
    }
    if !valid_token(&session_id, 160) {
        return Err(WorkspaceError::InvalidIdentity("session identity"));
    }
    if !valid_token(&vm_id, 63) {
        return Err(WorkspaceError::InvalidIdentity("VM identity"));
    }
    Ok(WorkspaceIdentity { app_id, session_id, vm_id })
}

fn read_identity(path: &Path, operation: &'static str) -> Result<String, WorkspaceError> {
    let raw = fs::read_to_string(path).map_err(|e| io_error(operation, e))?;
    Ok(raw.trim().trim_matches('"').to_string())
}

fn valid_app_id(value: &str) -> bool {
    (3..=255).contains(&value.len())
        && value.contains('.')
        && !value.starts_with('.')
        && !value.ends_with('.')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

fn valid_token(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'.' | b'-' | b'_'))
}

/// Run the immutable validator, then re-enter this binary inside one private
/// D-Bus session. No catalog value is accepted as a command, path, or variable.
pub fn run(paths: &WorkspacePaths, executable: &Path) -> Result<ExitStatus, WorkspaceError> {
    let status = Command::new(&paths.validator)
        .status()
        .map_err(|e| io_error("run App VM validator", e))?;
    if !status.success() {
        return Err(WorkspaceError::AdmissionRejected(status));
    }
    let status = Command::new(&paths.dbus_run_session)
        .arg("--")
        .arg(executable)
        .arg("--session-child")
        .status()
        .map_err(|e| io_error("start private D-Bus session", e))?;
    if status.success() {
        Ok(status)
    } else {
        Err(WorkspaceError::SessionFailed(status))
    }
}

/// Run the compositor and curated app inside the already-created D-Bus session.
pub fn run_session(paths: &WorkspacePaths) -> Result<ExitStatus, WorkspaceError> {
    let identity = load_identity(paths)?;
    prepare_private_dir(&paths.runtime_root)?;
    let config = paths.runtime_root.join("sway.conf");
    write_compositor_config(&config)?;

    let terminate = Arc::new(AtomicBool::new(false));
    for signal in [SIGTERM, SIGINT, SIGHUP] {
        flag::register(signal, Arc::clone(&terminate))
            .map_err(|e| io_error("register shutdown signal", e))?;
    }

    let mut compositor = Command::new(&paths.compositor)
        .args([OsStr::new("--unsupported-gpu"), OsStr::new("--config")])
        .arg(&config)
        .env("XDG_RUNTIME_DIR", &paths.runtime_root)
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| io_error("start guest compositor", e))?;

    let sockets = wait_for_sockets(paths, &mut compositor)?;
    let mut app = Command::new(&paths.flatpak)
        .args(["run", "--system", "curated"])
        .arg(&identity.app_id)
        .env("XDG_RUNTIME_DIR", &paths.runtime_root)
        .env("WAYLAND_DISPLAY", &sockets.wayland_display)
        .env("SWAYSOCK", &sockets.sway_socket)
        .spawn()
        .map_err(|e| io_error("start curated Flatpak", e))?;
    publish(paths, &identity, "connected", "application process started")?;

    let status = loop {
        if terminate.load(Ordering::Relaxed) {
            terminate_child(&mut app);
        }
        if let Some(status) = app
            .try_wait()
            .map_err(|e| io_error("observe curated Flatpak", e))?
        {
            break status;
        }
        if let Some(status) = compositor
            .try_wait()
            .map_err(|e| io_error("observe guest compositor", e))?
        {
            terminate_child(&mut app);
            let _ = app.wait();
            publish(paths, &identity, "failed", "guest compositor exited")?;
            return Err(WorkspaceError::CompositorExited(status));
        }
        std::thread::sleep(Duration::from_millis(100));
    };

    let reason = format!("application process exited with status {status}");
    publish(paths, &identity, "failed", &reason)?;
    let _ = Command::new(&paths.swaymsg)
        .arg("exit")
        .env("XDG_RUNTIME_DIR", &paths.runtime_root)
        .env("SWAYSOCK", &sockets.sway_socket)
        .status();
    terminate_child(&mut compositor);
    let _ = compositor.wait();
    Ok(status)
}

fn prepare_private_dir(path: &Path) -> Result<(), WorkspaceError> {
    fs::create_dir_all(path).map_err(|e| io_error("create private Wayland runtime", e))?;
    let metadata = fs::symlink_metadata(path).map_err(|e| io_error("inspect runtime", e))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(WorkspaceError::InvalidIdentity("runtime directory"));
    }
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|e| io_error("protect Wayland runtime", e))
}

fn write_compositor_config(path: &Path) -> Result<(), WorkspaceError> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| io_error("create compositor config", e))?;
    writeln!(
        file,
        "default_border none\ndefault_floating_border none\noutput * enable\noutput * mode {OUTPUT_WIDTH}x{OUTPUT_HEIGHT}\n"
    )
    .map_err(|e| io_error("write compositor config", e))
}

#[derive(Debug)]
struct Sockets {
    wayland_display: String,
    sway_socket: PathBuf,
}

fn wait_for_sockets(paths: &WorkspacePaths, compositor: &mut Child) -> Result<Sockets, WorkspaceError> {
    let deadline = Instant::now() + START_TIMEOUT;
    while Instant::now() < deadline {
        if let Some(status) = compositor
            .try_wait()
            .map_err(|e| io_error("observe guest compositor startup", e))?
        {
            return Err(WorkspaceError::CompositorExited(status));
        }
        let entries = fs::read_dir(&paths.runtime_root)
            .map_err(|e| io_error("inspect Wayland runtime sockets", e))?;
        let mut wayland_display = None;
        let mut sway_socket = None;
        for entry in entries {
            let entry = entry.map_err(|e| io_error("inspect Wayland socket", e))?;
            let name = entry.file_name();
            let text = name.to_string_lossy();
            let file_type = entry
                .file_type()
                .map_err(|e| io_error("inspect Wayland socket type", e))?;
            if file_type.is_socket() && text.starts_with("wayland-") {
                wayland_display = Some(text.into_owned());
            } else if file_type.is_socket() && text.starts_with("sway-ipc.") {
                sway_socket = Some(entry.path());
            }
        }
        if let (Some(wayland_display), Some(sway_socket)) = (wayland_display, sway_socket) {
            return Ok(Sockets { wayland_display, sway_socket });
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    terminate_child(compositor);
    Err(WorkspaceError::CompositorTimeout)
}

fn terminate_child(child: &mut Child) {
    let _ = Command::new("/usr/bin/kill")
        .args(["-TERM", &child.id().to_string()])
        .status();
}

#[derive(Serialize)]
struct RuntimeEvidence<'a> {
    session_id: &'a str,
    vm_id: &'a str,
    app_id: &'a str,
    generation: u64,
    state: &'a str,
    reason: &'a str,
}

fn publish(
    paths: &WorkspacePaths,
    identity: &WorkspaceIdentity,
    state: &str,
    reason: &str,
) -> Result<(), WorkspaceError> {
    let generation = next_generation(&paths.generation)?;
    let body = serde_json::to_string(&RuntimeEvidence {
        session_id: &identity.session_id,
        vm_id: &identity.vm_id,
        app_id: &identity.app_id,
        generation,
        state,
        reason,
    })
    .map_err(|e| io_error("serialize runtime evidence", io::Error::other(e)))?;
    if paths.bus.exists() {
        let _ = Command::new(&paths.bus)
            .args(["publish", "state/vdi/app-runtime", "--body-flag", &body])
            .status();
    }
    Ok(())
}

fn next_generation(path: &Path) -> Result<u64, WorkspaceError> {
    let current = match fs::read_to_string(path) {
        Ok(raw) => raw.trim().parse::<u64>().map_err(|_| WorkspaceError::BadGeneration)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
        Err(error) => return Err(io_error("read runtime generation", error)),
    };
    let next = current.checked_add(1).ok_or(WorkspaceError::BadGeneration)?;
    let parent = path.parent().ok_or(WorkspaceError::BadGeneration)?;
    fs::create_dir_all(parent).map_err(|e| io_error("create evidence directory", e))?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, format!("{next}\n"))
        .map_err(|e| io_error("write runtime generation", e))?;
    fs::rename(temporary, path).map_err(|e| io_error("commit runtime generation", e))?;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_loader_accepts_only_bounded_values() {
        let temp = tempfile::tempdir().expect("tempdir");
        let input = temp.path().join("input");
        fs::create_dir(&input).expect("input");
        fs::write(input.join("app-id"), "\"org.example.Editor\"\n").expect("app");
        fs::write(input.join("session-id"), "session:app-1\n").expect("session");
        let hostname = temp.path().join("hostname");
        fs::write(&hostname, "app-vm-1\n").expect("host");
        let paths = WorkspacePaths { input_root: input, hostname, ..WorkspacePaths::default() };
        let id = load_identity(&paths).expect("identity");
        assert_eq!(id.app_id, "org.example.Editor");

        fs::write(paths.input_root.join("app-id"), "org.example.App;reboot\n").expect("bad");
        assert!(matches!(load_identity(&paths), Err(WorkspaceError::InvalidIdentity(_))));
    }

    #[test]
    fn compositor_config_is_fixed_1080p_and_command_free() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("sway.conf");
        write_compositor_config(&path).expect("config");
        let value = fs::read_to_string(path).expect("read");
        assert!(value.contains("output * mode 1920x1080"));
        assert!(!value.contains("exec"));
        assert!(!value.contains("flatpak"));
    }

    #[test]
    fn evidence_generation_is_monotonic_and_rejects_corruption() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("state/generation");
        assert_eq!(next_generation(&path).expect("first"), 1);
        assert_eq!(next_generation(&path).expect("second"), 2);
        fs::write(&path, "not-a-number\n").expect("corrupt");
        assert!(matches!(next_generation(&path), Err(WorkspaceError::BadGeneration)));
    }

    #[test]
    fn private_runtime_rejects_symlink() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("target");
        fs::create_dir(&target).expect("target");
        let link = temp.path().join("runtime");
        std::os::unix::fs::symlink(target, &link).expect("link");
        assert!(matches!(prepare_private_dir(&link), Err(WorkspaceError::InvalidIdentity(_))));
    }
}
