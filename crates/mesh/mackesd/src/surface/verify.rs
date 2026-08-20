//! SURFACE-4 — per-subsystem verify probes + the compact fleet publish.
//!
//! The day-2 *verify* half of the Microsoft Surface enablement epic
//! (design: `docs/design/surface-tablet-enablement.md`, locks #5 + #7).
//! SURFACE-2's [`crate::surface`] detection folds the DMI identity into a
//! per-model [`SurfaceProfile`] — the checklist of subsystems the model
//! *has*. This unit probes exactly that checklist and folds each raw
//! reading into a **tri-state board** ([`ProbeState::Ok`] /
//! [`ProbeState::Degraded`] / [`ProbeState::Failed`], plus
//! [`ProbeState::NeedsGesture`] for the interactive probes) — each row
//! carrying a real reason string (the OW-10 self-test idiom, lock #5).
//!
//! **Only the subsystems the profile claims are probed.** A clamshell
//! Laptop has no detachable Type Cover, so that row never appears — verify
//! neither probes nor faults it. A `Failed` row is honestly red, never a
//! fake green.
//!
//! **Every reading comes through the injectable [`SurfaceProbes`] seam.**
//! The production seam ([`LiveSurfaceProbes`]) reads `/sys` / evdev directly
//! and uses fixed-argv, bounded, read-only libcamera and fprintd inventory
//! commands (§9 — no `dmidecode` or shell). It never opens a camera capture
//! stream, reads enrolled-print data, claims a fingerprint device, or starts
//! enrollment/authentication. The probes verify that each userspace stack can
//! enumerate a usable device without crossing those privacy boundaries (§7 —
//! the same discipline
//! [`super::enable::LiveSurfaceActions`] uses). Interactive-gesture probes
//! (pen pressure/tilt, S0ix suspend residency) fold to
//! [`ProbeState::NeedsGesture`] — an honest operator prompt, not a fault.
//! The pure classification folds (reading → tri-state) are unit-tested with
//! fixtures; the live reads remain environment-dependent integration probes.
//!
//! Alongside the full board this unit publishes the **compact
//! `state/hardware/surface/<node>` summary** (model, enablement %, count of
//! red subsystems) the Controller/fleet rollup reads (lock #7 — visibility
//! only, never remote control). §6-clean: it stays wholly in mackesd.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::{Subsystem, SurfaceDetection, SurfaceDevice, SurfaceModel, SurfaceProfile};
use mackes_mesh_types::surface_hardware::{
    SurfaceAvailability, SurfaceCameraProofFailure, SurfaceCameraProofOutcome,
    SurfaceCameraProofUnavailable, SurfaceFleetSummary, SurfaceModelIdentity,
    SurfaceObservationSource, SurfaceProGeneration, SurfaceProbeState, SurfaceProbeVerdict,
    SurfacePublication, SurfaceSubsystem, SurfaceVerifyBoard, SURFACE_HARDWARE_SCHEMA_VERSION,
};

// ─────────────────────────────── the tri-state ──────────────────────────────

/// One subsystem's verify verdict — the board's cell state (lock #5).
///
/// The tri-state is `Ok`/`Degraded`/`Failed`; the interactive-gesture probes
/// (pen, suspend) add [`Self::NeedsGesture`], which prompts the operator
/// honestly rather than faulting a subsystem we simply haven't exercised yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeState {
    /// Verified working — the reading confirms the subsystem is live.
    Ok,
    /// Present but not fully healthy — one of several expected signals is
    /// missing (SAM battery readable but thermal not; pen pressure but no
    /// tilt; a camera device enumerated but its libcamera pipeline unavailable).
    Degraded,
    /// The subsystem the profile says the model *has* is absent or broken —
    /// honestly red, never a fake green.
    Failed,
    /// The probe needs an operator gesture to complete (a pen stroke, a
    /// suspend/resume cycle). Not a fault — an honest prompt.
    NeedsGesture,
}

impl ProbeState {
    /// Stable identifier for state keys / logs / the fleet summary.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Degraded => "degraded",
            Self::Failed => "failed",
            Self::NeedsGesture => "needs_gesture",
        }
    }

    /// Does this state count as a **red** subsystem for the fleet rollup?
    /// Only an outright [`Self::Failed`] is red — a `Degraded` or a pending
    /// gesture is not a fleet-health alarm (lock #7 "any red subsystem").
    #[must_use]
    pub const fn is_red(self) -> bool {
        matches!(self, Self::Failed)
    }

    /// Does this state count toward the enablement percentage? Only a fully
    /// [`Self::Ok`] subsystem is counted enabled.
    #[must_use]
    pub const fn is_enabled(self) -> bool {
        matches!(self, Self::Ok)
    }
}

// ─────────────────────────────── raw readings ───────────────────────────────

/// A raw evdev/sysfs presence reading for a keyboard/touch device (touch,
/// Type Cover). Best-effort, like every mackesd probe: a missing device is
/// `present: false`, never an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputPresence {
    /// A matching input device was enumerated under `/sys/class/input`.
    pub present: bool,
    /// The matched device's kernel `name` (for the board's detail line).
    pub name: String,
}

/// The active pen digitizer reading. Pressure + tilt only appear once the
/// operator actually touches the pen to the screen — hence the gesture path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PenReading {
    /// The pen digitizer is enumerated.
    pub digitizer_present: bool,
    /// A non-zero pressure sample was observed.
    pub pressure_seen: bool,
    /// A tilt (X/Y) sample was observed.
    pub tilt_seen: bool,
}

/// The Surface Aggregator Module reading — battery + thermal readability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SamReading {
    /// A `/sys/class/power_supply` capacity was readable.
    pub battery_readable: bool,
    /// A `/sys/class/thermal` zone temperature was readable.
    pub thermal_readable: bool,
}

/// The accelerometer reading — an orientation vector, if the IIO device
/// yields one.
// `f64` axes are not `Eq` (NaN) — `PartialEq` is the honest bound for a raw
// float reading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AccelReading {
    /// The three raw axes (x, y, z), if the IIO accelerometer reported them.
    pub vector: Option<[f64; 3]>,
}

/// The camera reading — device enumerated + non-capturing pipeline readiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CameraReading {
    /// A V4L2 capture device is enumerated.
    pub device_present: bool,
    /// libcamera exposed a capture pipeline. No frame was requested or read.
    pub pipeline_ready: bool,
}

/// The Wi-Fi + Bluetooth reading — each radio's up/down state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WifiBtReading {
    /// A wireless netdev is present and up.
    pub wifi_up: bool,
    /// A Bluetooth controller is present and up.
    pub bt_up: bool,
}

/// The S0ix modern-standby reading — the residency counter, and whether it
/// advanced across a suspend (the gesture confirmation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct S0ixReading {
    /// The S0ix residency counter's current value, if the counter exists.
    pub residency_counter: Option<u64>,
    /// Whether the counter advanced across a suspend/resume: `None` until a
    /// suspend cycle is measured (the gesture), `Some(true/false)` after.
    pub advanced: Option<bool>,
}

/// The fingerprint reader reading — device present + read-only stack readiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FingerprintReading {
    /// A fingerprint device is enumerated.
    pub device_present: bool,
    /// fprintd's manager exposed the device without claiming it.
    pub stack_ready: bool,
}

// ─────────────────────────────── the seam ───────────────────────────────────

/// A typed failure from the [`SurfaceProbes`] seam — mirrors
/// [`super::enable::EnableError`]'s honest split.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeError {
    /// The live probe isn't wired to real hardware yet — the honest answer
    /// on any non-Surface dev box / headless CI (§7: never a faked green).
    /// `probe` names what was gated (e.g. `"camera frame capture"`).
    IntegrationGated {
        /// The probe that is integration-gated.
        probe: String,
    },
    /// The live probe ran and failed for a concrete reason.
    Failed {
        /// The probe that failed.
        probe: String,
        /// The underlying reason.
        detail: String,
    },
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IntegrationGated { probe } => {
                write!(f, "{probe}: integration-gated (live Surface hardware)")
            }
            Self::Failed { probe, detail } => write!(f, "{probe}: {detail}"),
        }
    }
}

impl std::error::Error for ProbeError {}

/// The injectable seam over every subsystem reading the verify board needs.
/// Tests hand a fixture; production hands [`LiveSurfaceProbes`].
///
/// Every method is fallible with a typed [`ProbeError`] so a gated/failed
/// read folds to an honest board cell — never a silent green.
///
/// # Errors
///
/// A probe may return [`ProbeError::IntegrationGated`] when its required live
/// gesture or hardware boundary is unavailable and [`ProbeError::Failed`] on a
/// concrete read failure; the classification folds turn either into an honest
/// red cell.
pub trait SurfaceProbes {
    /// Read the capacitive touchscreen's evdev presence.
    fn probe_touch(&self) -> Result<InputPresence, ProbeError>;
    /// Read the active pen digitizer (pressure/tilt need a gesture).
    fn probe_pen(&self) -> Result<PenReading, ProbeError>;
    /// Read whether the detachable Type Cover is enumerated.
    fn probe_type_cover(&self) -> Result<InputPresence, ProbeError>;
    /// Read the Surface Aggregator battery + thermal readability.
    fn probe_sam(&self) -> Result<SamReading, ProbeError>;
    /// Read the accelerometer's orientation vector.
    fn probe_accelerometer(&self) -> Result<AccelReading, ProbeError>;
    /// Read the camera's non-capturing libcamera pipeline capability.
    fn probe_camera(&self) -> Result<CameraReading, ProbeError>;
    /// Read the Wi-Fi + Bluetooth radios' up/down state.
    fn probe_wifi_bt(&self) -> Result<WifiBtReading, ProbeError>;
    /// Read the S0ix residency counter (advancement needs a suspend gesture).
    fn probe_s0ix(&self) -> Result<S0ixReading, ProbeError>;
    /// Read the fingerprint reader's non-claiming fprintd capability.
    fn probe_fingerprint(&self) -> Result<FingerprintReading, ProbeError>;
}

// ─────────────────────────── the production seam ────────────────────────────

/// The production seam. §9-clean: it reads `/sys` / evdev directly and runs
/// only fixed-argv, bounded, read-only libcamera/fprintd inventory commands
/// (no `dmidecode`/shell). Camera frame capture and fingerprint claim,
/// enrollment, verification, and enrolled-print listing are deliberately
/// absent. The interactive fields (pen pressure/tilt, S0ix advancement) come
/// back unset so the fold prompts the operator (a gesture), never green.
#[derive(Debug, Clone, Copy, Default)]
pub struct LiveSurfaceProbes;

impl LiveSurfaceProbes {
    const INPUT_DIR: &'static str = "/sys/class/input";
    const POWER_DIR: &'static str = "/sys/class/power_supply";
    const THERMAL_DIR: &'static str = "/sys/class/thermal";
    const IIO_DIR: &'static str = "/sys/bus/iio/devices";
    const NET_DIR: &'static str = "/sys/class/net";
    const BT_DIR: &'static str = "/sys/class/bluetooth";
    const LIBCAMERA_CAM: &'static str = "/usr/bin/cam";
    const LIBCAMERA_ARGS: [&'static str; 1] = ["--list"];
    const LIBCAMERA_TIMEOUT: Duration = Duration::from_secs(5);
    const LIBCAMERA_OUTPUT_LIMIT: usize = 64 * 1024;
    const BUSCTL: &'static str = "/usr/bin/busctl";
    const FPRINT_MANAGER_ARGS: [&'static str; 7] = [
        "--system",
        "--timeout=5",
        "call",
        "net.reactivated.Fprint",
        "/net/reactivated/Fprint/Manager",
        "net.reactivated.Fprint.Manager",
        "GetDevices",
    ];
    const FPRINT_TIMEOUT: Duration = Duration::from_secs(5);
    const FPRINT_OUTPUT_LIMIT: usize = 16 * 1024;
    /// A representative Intel PMC S0ix residency counter (µs since boot).
    const S0IX_RESIDENCY: &'static str = "/sys/kernel/debug/pmc_core/slp_s0_residency_usec";

    /// Read a `/sys` scalar file, trimmed, if present.
    fn scalar(path: &Path) -> Option<String> {
        std::fs::read_to_string(path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Find the first enumerated `/sys/class/input/input*/name` whose value
    /// (lowercased) contains any of `needles`. Returns the matched name.
    fn input_named(needles: &[&str]) -> Option<String> {
        let entries = std::fs::read_dir(Self::INPUT_DIR).ok()?;
        for entry in entries.flatten() {
            let name_path = entry.path().join("name");
            if let Some(name) = Self::scalar(&name_path) {
                let lower = name.to_lowercase();
                if needles.iter().any(|n| lower.contains(n)) {
                    return Some(name);
                }
            }
        }
        None
    }

    /// Is any entry in `dir` a directory (best-effort presence check)?
    fn any_dir_entry(dir: &str) -> bool {
        std::fs::read_dir(dir)
            .map(|mut d| d.next().is_some())
            .unwrap_or(false)
    }

    /// Enumerate cameras through libcamera without opening a capture stream.
    /// The fixed command is time- and output-bounded and never invokes a
    /// shell. Frame capture remains a separately armed/privacy-gated action.
    fn enumerate_libcamera() -> Result<bool, ProbeError> {
        let output = run_bounded_command(
            Self::LIBCAMERA_CAM,
            &Self::LIBCAMERA_ARGS,
            Self::LIBCAMERA_TIMEOUT,
            Self::LIBCAMERA_OUTPUT_LIMIT,
        )
        .map_err(|detail| ProbeError::Failed {
            probe: "libcamera enumeration".to_string(),
            detail,
        })?;

        parse_libcamera_list(&output).map_err(|detail| ProbeError::Failed {
            probe: "libcamera enumeration".to_string(),
            detail,
        })
    }

    /// Ask fprintd's read-only Manager.GetDevices method for attached readers.
    /// This neither claims a reader nor requests enrolled-print information.
    fn enumerate_fprintd() -> Result<Vec<String>, ProbeError> {
        let output = run_bounded_command(
            Self::BUSCTL,
            &Self::FPRINT_MANAGER_ARGS,
            Self::FPRINT_TIMEOUT,
            Self::FPRINT_OUTPUT_LIMIT,
        )
        .map_err(|detail| ProbeError::Failed {
            probe: "fprintd device inventory".to_string(),
            detail,
        })?;

        parse_fprintd_devices(&output).map_err(|detail| ProbeError::Failed {
            probe: "fprintd device inventory".to_string(),
            detail,
        })
    }
}

/// Run one fixed-argv observation command with bounded wall time and output.
/// Both pipes are drained concurrently so a noisy program cannot deadlock the
/// daemon. Output beyond `limit` is rejected rather than parsed partially.
fn run_bounded_command(
    program: &str,
    args: &[&str],
    timeout: Duration,
    limit: usize,
) -> Result<String, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start {program}: {e}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("could not capture {program} stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("could not capture {program} stderr"))?;
    let stdout_reader = std::thread::spawn(move || read_bounded(stdout, limit));
    let stderr_reader = std::thread::spawn(move || read_bounded(stderr, limit));

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {} seconds", timeout.as_secs()));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("could not wait for {program}: {e}"));
            }
        }
    };

    let (stdout, stdout_overflow) = stdout_reader
        .join()
        .map_err(|_| format!("{program} stdout reader failed"))?
        .map_err(|e| format!("could not read {program} stdout: {e}"))?;
    let (stderr, stderr_overflow) = stderr_reader
        .join()
        .map_err(|_| format!("{program} stderr reader failed"))?
        .map_err(|e| format!("could not read {program} stderr: {e}"))?;

    if stdout_overflow || stderr_overflow {
        return Err(format!("output exceeded {limit} bytes"));
    }
    let stderr = String::from_utf8_lossy(&stderr);
    if !status.success() {
        let detail = stderr.trim();
        return Err(if detail.is_empty() {
            format!("exited with {status}")
        } else {
            format!("exited with {status}: {detail}")
        });
    }
    String::from_utf8(stdout).map_err(|_| "stdout was not valid UTF-8".to_string())
}

fn read_bounded(mut reader: impl Read, limit: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let mut output = Vec::with_capacity(limit.min(4096));
    reader
        .by_ref()
        .take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut output)?;
    let overflow = output.len() > limit;
    output.truncate(limit);
    Ok((output, overflow))
}

/// Typed provider seam for the separately armed camera functional proof.
pub trait CameraFunctionalProofProvider: Send + Sync {
    /// Exercise exactly one camera frame without retaining its bytes.
    fn prove_one_frame(&self) -> SurfaceCameraProofOutcome;
}

/// Production libcamera provider. Unlike the enumeration probe, this opens a
/// stream and therefore may only be reached after action authorization and the
/// explicit operator phrase have both passed.
#[derive(Debug, Clone, Copy, Default)]
pub struct LiveCameraFunctionalProofProvider;

impl LiveCameraFunctionalProofProvider {
    const PROGRAM: &'static str = "/usr/bin/cam";
    // Fedora 44 libcamera-tools 0.7.1 implements a positive numeric camera
    // selector as one-based (`cameras[index - 1]`). Selecting `1` therefore
    // means the first available camera, not an assumed second camera and not
    // a parsed/retained device id. Its capture count is an exact frame limit;
    // a filename without `#` remains unchanged, so the sole sink is /dev/null.
    const ARGS: [&'static str; 3] = ["--camera=1", "--capture=1", "--file=/dev/null"];
    const TIMEOUT: Duration = Duration::from_secs(8);
}

impl CameraFunctionalProofProvider for LiveCameraFunctionalProofProvider {
    fn prove_one_frame(&self) -> SurfaceCameraProofOutcome {
        let mut child = match Command::new(Self::PROGRAM)
            .args(Self::ARGS)
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin")
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return SurfaceCameraProofOutcome::Unavailable(
                    SurfaceCameraProofUnavailable::ProviderMissing,
                );
            }
            Err(_) => {
                return SurfaceCameraProofOutcome::Failed(SurfaceCameraProofFailure::CaptureFailed);
            }
        };

        let deadline = Instant::now() + Self::TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => {
                    return SurfaceCameraProofOutcome::Passed;
                }
                Ok(Some(_)) | Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return SurfaceCameraProofOutcome::Failed(
                        SurfaceCameraProofFailure::CaptureFailed,
                    );
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return SurfaceCameraProofOutcome::Failed(SurfaceCameraProofFailure::TimedOut);
                }
            }
        }
    }
}

/// Parse `cam --list` conservatively. A successful command with the expected
/// header and no numbered entries truthfully means no libcamera cameras. Any
/// unfamiliar output is an error, not an inferred success.
fn parse_libcamera_list(output: &str) -> Result<bool, String> {
    if output
        .bytes()
        .any(|byte| byte.is_ascii_control() && byte != b'\n' && byte != b'\t')
    {
        return Err("output contained control characters".to_string());
    }
    let mut saw_header = false;
    let mut camera_indexes = [false; 16];
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed == "Available cameras:" {
            if saw_header {
                return Err("unexpected output (duplicate camera header)".to_string());
            }
            saw_header = true;
            continue;
        }
        if saw_header {
            if trimmed.is_empty() {
                continue;
            }
            let (index, description) = trimmed
                .split_once(':')
                .ok_or_else(|| "unexpected camera row".to_string())?;
            let index = index
                .parse::<usize>()
                .map_err(|_| "invalid camera index".to_string())?;
            if index >= camera_indexes.len() {
                return Err("camera index exceeded 15".to_string());
            }
            if camera_indexes[index] {
                return Err("duplicate camera index".to_string());
            }
            let description = description.trim();
            if description.len() > 1024
                || !description.contains(" (")
                || !description.ends_with(')')
            {
                return Err("malformed camera description".to_string());
            }
            camera_indexes[index] = true;
        }
    }
    if !saw_header {
        return Err("unexpected output (missing `Available cameras:` header)".to_string());
    }
    Ok(camera_indexes.into_iter().any(|seen| seen))
}

/// Parse `busctl ... Manager.GetDevices` output exactly enough to reject
/// malformed or injected object paths. The expected shape is `ao N "PATH"…`.
/// fprintd device object paths contain no biometric or enrollment information.
fn parse_fprintd_devices(output: &str) -> Result<Vec<String>, String> {
    const PREFIX: &str = "/net/reactivated/Fprint/Device/";
    const MAX_DEVICES: usize = 16;

    let body = output.strip_suffix('\n').unwrap_or(output);
    if body.chars().any(char::is_control) {
        return Err("output contained control characters".to_string());
    }
    let tokens: Vec<_> = body.split_ascii_whitespace().collect();
    if tokens.len() < 2 || tokens[0] != "ao" {
        return Err("unexpected output (expected `ao` object-path array)".to_string());
    }
    let count = tokens[1]
        .parse::<usize>()
        .map_err(|_| "invalid device count".to_string())?;
    if tokens[1] != count.to_string() {
        return Err("device count was not canonical decimal".to_string());
    }
    if count > MAX_DEVICES {
        return Err(format!("device count exceeded {MAX_DEVICES}"));
    }
    if tokens.len() != count.saturating_add(2) {
        return Err("device count did not match object paths".to_string());
    }

    tokens[2..]
        .iter()
        .map(|token| {
            let path = token
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .ok_or_else(|| "object path was not quoted".to_string())?;
            let suffix = path
                .strip_prefix(PREFIX)
                .ok_or_else(|| "object path was outside fprintd device namespace".to_string())?;
            if suffix.is_empty()
                || !suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            {
                return Err("object path contained invalid device id".to_string());
            }
            Ok(path.to_string())
        })
        .collect()
}

impl SurfaceProbes for LiveSurfaceProbes {
    fn probe_touch(&self) -> Result<InputPresence, ProbeError> {
        let name = Self::input_named(&["touchscreen", "ipts", "touch"]);
        Ok(InputPresence {
            present: name.is_some(),
            name: name.unwrap_or_default(),
        })
    }

    fn probe_pen(&self) -> Result<PenReading, ProbeError> {
        // The digitizer enumerates as a sysfs input; pressure/tilt samples
        // only exist once the pen touches the screen — a live evdev grab is
        // the gesture, so headless the fold prompts for it (never green).
        let present = Self::input_named(&["pen", "stylus", "digitizer"]).is_some();
        Ok(PenReading {
            digitizer_present: present,
            pressure_seen: false,
            tilt_seen: false,
        })
    }

    fn probe_type_cover(&self) -> Result<InputPresence, ProbeError> {
        let name = Self::input_named(&["type cover", "surface type", "cover keyboard"]);
        Ok(InputPresence {
            present: name.is_some(),
            name: name.unwrap_or_default(),
        })
    }

    fn probe_sam(&self) -> Result<SamReading, ProbeError> {
        let battery_readable = std::fs::read_dir(Self::POWER_DIR)
            .map(|entries| {
                entries
                    .flatten()
                    .any(|e| Self::scalar(&e.path().join("capacity")).is_some())
            })
            .unwrap_or(false);
        let thermal_readable = std::fs::read_dir(Self::THERMAL_DIR)
            .map(|entries| {
                entries
                    .flatten()
                    .any(|e| Self::scalar(&e.path().join("temp")).is_some())
            })
            .unwrap_or(false);
        Ok(SamReading {
            battery_readable,
            thermal_readable,
        })
    }

    fn probe_accelerometer(&self) -> Result<AccelReading, ProbeError> {
        let read_axis = |dev: &Path, axis: &str| -> Option<f64> {
            Self::scalar(&dev.join(format!("in_accel_{axis}_raw")))?
                .parse::<f64>()
                .ok()
        };
        let vector = std::fs::read_dir(Self::IIO_DIR).ok().and_then(|entries| {
            entries.flatten().find_map(|e| {
                let dev = e.path();
                Some([
                    read_axis(&dev, "x")?,
                    read_axis(&dev, "y")?,
                    read_axis(&dev, "z")?,
                ])
            })
        });
        Ok(AccelReading { vector })
    }

    fn probe_camera(&self) -> Result<CameraReading, ProbeError> {
        // libcamera is the authoritative enumeration path for the IPU3 camera
        // stack used by Surface Pro 5/6. Enumeration has no capture side
        // effect; frame capture remains privacy-gated and is never attempted.
        let pipeline_ready = Self::enumerate_libcamera()?;
        Ok(CameraReading {
            device_present: pipeline_ready,
            pipeline_ready,
        })
    }

    fn probe_wifi_bt(&self) -> Result<WifiBtReading, ProbeError> {
        let wifi_up = std::fs::read_dir(Self::NET_DIR)
            .map(|entries| {
                entries.flatten().any(|e| {
                    let dev = e.path();
                    // A wireless netdev has a `wireless`/`phy80211` dir; "up"
                    // is operstate == up.
                    let is_wireless =
                        dev.join("wireless").exists() || dev.join("phy80211").exists();
                    is_wireless && Self::scalar(&dev.join("operstate")).as_deref() == Some("up")
                })
            })
            .unwrap_or(false);
        let bt_up = Self::any_dir_entry(Self::BT_DIR);
        Ok(WifiBtReading { wifi_up, bt_up })
    }

    fn probe_s0ix(&self) -> Result<S0ixReading, ProbeError> {
        // The residency counter is a plain scalar; whether it *advances*
        // needs a suspend/resume (the gesture) — left `None` so the fold
        // prompts for it rather than guessing.
        let residency_counter =
            Self::scalar(Path::new(Self::S0IX_RESIDENCY)).and_then(|s| s.parse::<u64>().ok());
        Ok(S0ixReading {
            residency_counter,
            advanced: None,
        })
    }

    fn probe_fingerprint(&self) -> Result<FingerprintReading, ProbeError> {
        // Manager.GetDevices is the narrowest fprintd capability probe: it
        // discovers libfprint-backed devices without Claim, enrolled-print
        // listing, enrollment, authentication, or raw biometric data access.
        let devices = Self::enumerate_fprintd()?;
        Ok(FingerprintReading {
            device_present: !devices.is_empty(),
            stack_ready: !devices.is_empty(),
        })
    }
}

// ─────────────────────────── the classification folds (pure) ────────────────

/// One subsystem's row on the verify board — the subsystem, its tri-state,
/// and the real reason string (lock #5: every cell carries a reason).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubsystemVerdict {
    /// The subsystem this row verifies.
    pub subsystem: Subsystem,
    /// Its tri-state (+ gesture) verdict.
    pub state: ProbeState,
    /// The honest reason behind the state.
    pub reason: String,
}

impl SubsystemVerdict {
    fn new(subsystem: Subsystem, state: ProbeState, reason: impl Into<String>) -> Self {
        Self {
            subsystem,
            state,
            reason: reason.into(),
        }
    }

    /// Fold a seam error into an honest verdict: a gated probe is red (we
    /// genuinely couldn't confirm it — never a fake green), a failed probe
    /// carries its detail.
    fn from_err(subsystem: Subsystem, err: &ProbeError) -> Self {
        Self::new(subsystem, ProbeState::Failed, err.to_string())
    }
}

/// Classify the touchscreen reading. Enumeration is the green bar (the iptsd
/// digitizer is present + bound); an absent device is red.
#[must_use]
pub fn classify_touch(reading: Result<InputPresence, ProbeError>) -> SubsystemVerdict {
    match reading {
        Ok(r) if r.present => SubsystemVerdict::new(
            Subsystem::Touch,
            ProbeState::Ok,
            format!("touchscreen enumerated ({})", r.name),
        ),
        Ok(_) => SubsystemVerdict::new(
            Subsystem::Touch,
            ProbeState::Failed,
            "no touchscreen input device enumerated",
        ),
        Err(e) => SubsystemVerdict::from_err(Subsystem::Touch, &e),
    }
}

/// Classify the pen reading.
///
/// Pressure + tilt are the green bar; pressure without tilt is degraded; an
/// enumerated digitizer with no samples yet prompts a gesture (touch the pen
/// to the screen); no digitizer is red.
#[must_use]
pub fn classify_pen(reading: Result<PenReading, ProbeError>) -> SubsystemVerdict {
    let r = match reading {
        Ok(r) => r,
        Err(e) => return SubsystemVerdict::from_err(Subsystem::Pen, &e),
    };
    let (state, reason) = if !r.digitizer_present {
        (
            ProbeState::Failed,
            "no pen digitizer enumerated".to_string(),
        )
    } else if r.pressure_seen && r.tilt_seen {
        (ProbeState::Ok, "pen reports pressure + tilt".to_string())
    } else if r.pressure_seen {
        (
            ProbeState::Degraded,
            "pen reports pressure but no tilt".to_string(),
        )
    } else {
        (
            ProbeState::NeedsGesture,
            "press the pen to the screen to confirm pressure/tilt".to_string(),
        )
    };
    SubsystemVerdict::new(Subsystem::Pen, state, reason)
}

/// Classify the Type Cover reading. Enumerated → green; detached/absent →
/// red (the profile claims the model *has* a Type Cover).
#[must_use]
pub fn classify_type_cover(reading: Result<InputPresence, ProbeError>) -> SubsystemVerdict {
    match reading {
        Ok(r) if r.present => SubsystemVerdict::new(
            Subsystem::TypeCover,
            ProbeState::Ok,
            format!("Type Cover enumerated ({})", r.name),
        ),
        Ok(_) => SubsystemVerdict::new(
            Subsystem::TypeCover,
            ProbeState::Failed,
            "Type Cover not enumerated (detached?)",
        ),
        Err(e) => SubsystemVerdict::from_err(Subsystem::TypeCover, &e),
    }
}

/// Classify the SAM reading. Battery **and** thermal readable → green; one of
/// the two → degraded; neither → red.
#[must_use]
pub fn classify_sam(reading: Result<SamReading, ProbeError>) -> SubsystemVerdict {
    let r = match reading {
        Ok(r) => r,
        Err(e) => return SubsystemVerdict::from_err(Subsystem::Sam, &e),
    };
    let (state, reason) = match (r.battery_readable, r.thermal_readable) {
        (true, true) => (ProbeState::Ok, "battery + thermal readable".to_string()),
        (true, false) => (
            ProbeState::Degraded,
            "battery readable but no thermal zone".to_string(),
        ),
        (false, true) => (
            ProbeState::Degraded,
            "thermal readable but no battery".to_string(),
        ),
        (false, false) => (
            ProbeState::Failed,
            "SAM battery + thermal both unreadable".to_string(),
        ),
    };
    SubsystemVerdict::new(Subsystem::Sam, state, reason)
}

/// Sane gravity band (raw-unit agnostic): a live accelerometer's vector
/// magnitude should be clearly non-zero. A zero/near-zero vector is a stuck
/// or absent sensor.
const ACCEL_MIN_MAGNITUDE: f64 = 1.0;

/// Classify the accelerometer reading. A plausible non-zero orientation
/// vector → green; a present-but-implausible (near-zero) vector → degraded;
/// no vector → red.
#[must_use]
pub fn classify_accelerometer(reading: Result<AccelReading, ProbeError>) -> SubsystemVerdict {
    let r = match reading {
        Ok(r) => r,
        Err(e) => return SubsystemVerdict::from_err(Subsystem::RotationAccel, &e),
    };
    let (state, reason) = match r.vector {
        None => (
            ProbeState::Failed,
            "no accelerometer orientation vector".to_string(),
        ),
        Some([x, y, z]) => {
            let magnitude = x.mul_add(x, y.mul_add(y, z * z)).sqrt();
            if magnitude >= ACCEL_MIN_MAGNITUDE {
                (
                    ProbeState::Ok,
                    format!("orientation vector ({x:.0}, {y:.0}, {z:.0})"),
                )
            } else {
                (
                    ProbeState::Degraded,
                    "accelerometer reads a near-zero (stuck?) vector".to_string(),
                )
            }
        }
    };
    SubsystemVerdict::new(Subsystem::RotationAccel, state, reason)
}

/// Classify the camera reading. A non-capturing libcamera pipeline → green;
/// a device without a ready pipeline → degraded; absent → red.
#[must_use]
pub fn classify_camera(reading: Result<CameraReading, ProbeError>) -> SubsystemVerdict {
    let r = match reading {
        Ok(r) => r,
        Err(e) => return SubsystemVerdict::from_err(Subsystem::Cameras, &e),
    };
    let (state, reason) = if !r.device_present {
        (ProbeState::Failed, "no camera enumerated".to_string())
    } else if r.pipeline_ready {
        (
            ProbeState::Ok,
            "libcamera pipeline ready (no frame captured)".to_string(),
        )
    } else {
        (
            ProbeState::Degraded,
            "camera enumerated but libcamera pipeline unavailable".to_string(),
        )
    };
    SubsystemVerdict::new(Subsystem::Cameras, state, reason)
}

/// Classify the Wi-Fi + Bluetooth reading. Both radios up → green; one up →
/// degraded; neither → red.
#[must_use]
pub fn classify_wifi_bt(reading: Result<WifiBtReading, ProbeError>) -> SubsystemVerdict {
    let r = match reading {
        Ok(r) => r,
        Err(e) => return SubsystemVerdict::from_err(Subsystem::WifiBt, &e),
    };
    let (state, reason) = match (r.wifi_up, r.bt_up) {
        (true, true) => (ProbeState::Ok, "Wi-Fi + Bluetooth up".to_string()),
        (true, false) => (ProbeState::Degraded, "Wi-Fi up, Bluetooth down".to_string()),
        (false, true) => (ProbeState::Degraded, "Bluetooth up, Wi-Fi down".to_string()),
        (false, false) => (
            ProbeState::Failed,
            "Wi-Fi + Bluetooth both down".to_string(),
        ),
    };
    SubsystemVerdict::new(Subsystem::WifiBt, state, reason)
}

/// Classify the S0ix reading.
///
/// A counter that advanced across a suspend → green; a counter that did
/// **not** advance → red (modern standby broken); a counter with no suspend
/// measured yet → gesture prompt; no counter → red.
#[must_use]
pub fn classify_s0ix(reading: Result<S0ixReading, ProbeError>) -> SubsystemVerdict {
    let r = match reading {
        Ok(r) => r,
        Err(e) => return SubsystemVerdict::from_err(Subsystem::S0ix, &e),
    };
    let (state, reason) = match (r.residency_counter, r.advanced) {
        (None, _) => (
            ProbeState::Failed,
            "no S0ix residency counter (modern standby unsupported?)".to_string(),
        ),
        (Some(_), Some(true)) => (
            ProbeState::Ok,
            "S0ix residency advanced across suspend".to_string(),
        ),
        (Some(_), Some(false)) => (
            ProbeState::Failed,
            "S0ix residency did not advance across suspend".to_string(),
        ),
        (Some(_), None) => (
            ProbeState::NeedsGesture,
            "suspend then resume to confirm S0ix residency advances".to_string(),
        ),
    };
    SubsystemVerdict::new(Subsystem::S0ix, state, reason)
}

/// Classify the fingerprint reading. Present through the read-only fprintd
/// inventory → green; present without a ready stack → degraded; absent → red.
#[must_use]
pub fn classify_fingerprint(reading: Result<FingerprintReading, ProbeError>) -> SubsystemVerdict {
    let r = match reading {
        Ok(r) => r,
        Err(e) => return SubsystemVerdict::from_err(Subsystem::Fingerprint, &e),
    };
    let (state, reason) = if !r.device_present {
        (
            ProbeState::Failed,
            "no fingerprint reader enumerated".to_string(),
        )
    } else if r.stack_ready {
        (
            ProbeState::Ok,
            "fingerprint reader enumerated through fprintd (not claimed)".to_string(),
        )
    } else {
        (
            ProbeState::Degraded,
            "fingerprint reader present but fprintd stack unavailable".to_string(),
        )
    };
    SubsystemVerdict::new(Subsystem::Fingerprint, state, reason)
}

// ─────────────────────────── the board (profile-gated) ──────────────────────

/// The full per-node verify board — the model string + one row per subsystem
/// the model's profile claims. SURFACE-6's Test tab renders it; [`summarize`]
/// folds it to the compact fleet summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyBoard {
    /// The recognised model's product string (empty when the node isn't a
    /// recognised Surface — then `rows` is empty and nothing is probed).
    pub model: String,
    /// When set, verify was skipped and this is the honest reason.
    pub skipped: Option<String>,
    /// One row per profile-claimed subsystem, in board order.
    pub rows: Vec<SubsystemVerdict>,
}

impl VerifyBoard {
    fn skipped(model: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            skipped: Some(reason.into()),
            rows: Vec::new(),
        }
    }
}

/// Probe exactly the subsystems `profile` claims, in board order, folding
/// each seam reading through its classification. The **profile gate**: a
/// line item the model doesn't have (a Laptop's Type Cover) never produces a
/// row — verify neither probes nor faults it (lock #5). Pure control flow
/// over the injectable seam.
fn probe_claimed(probes: &impl SurfaceProbes, profile: &SurfaceProfile) -> Vec<SubsystemVerdict> {
    profile
        .expected()
        .into_iter()
        .map(|subsystem| match subsystem {
            Subsystem::Touch => classify_touch(probes.probe_touch()),
            Subsystem::Pen => classify_pen(probes.probe_pen()),
            Subsystem::TypeCover => classify_type_cover(probes.probe_type_cover()),
            Subsystem::Sam => classify_sam(probes.probe_sam()),
            Subsystem::RotationAccel => classify_accelerometer(probes.probe_accelerometer()),
            Subsystem::Cameras => classify_camera(probes.probe_camera()),
            Subsystem::WifiBt => classify_wifi_bt(probes.probe_wifi_bt()),
            Subsystem::S0ix => classify_s0ix(probes.probe_s0ix()),
            Subsystem::Fingerprint => classify_fingerprint(probes.probe_fingerprint()),
        })
        .collect()
}

/// The `surface_verify` verb: probe this node's claimed subsystems and fold
/// them into the board.
///
/// A non-Surface (or unrecognised-Surface) node is skipped cleanly — no
/// probes, an honest `skipped` reason, no rows.
#[must_use]
pub fn run_verify(probes: &impl SurfaceProbes, detection: &SurfaceDetection) -> VerifyBoard {
    let device: &SurfaceDevice = match &detection.model {
        SurfaceModel::NotASurface => {
            return VerifyBoard::skipped("", "not a Microsoft Surface");
        }
        SurfaceModel::UnknownSurface { product } => {
            return VerifyBoard::skipped(
                product.clone(),
                format!("unrecognised Surface: {product} (no per-model profile)"),
            );
        }
        SurfaceModel::Known(dev) => dev,
    };

    VerifyBoard {
        model: device.product.clone(),
        skipped: None,
        rows: probe_claimed(probes, &device.profile),
    }
}

// ─────────────────────────── the compact fleet summary ──────────────────────

/// The compact `state/hardware/surface/<node>` summary the fleet rollup reads
/// (lock #7): model, enablement %, and the red subsystems. Visibility only —
/// no remote control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetSummary {
    /// The publishing node's id.
    pub node: String,
    /// The recognised model's product string.
    pub model: String,
    /// Percent of claimed subsystems that verified fully green (0–100).
    pub enablement_pct: u8,
    /// The number of red ([`ProbeState::Failed`]) subsystems.
    pub red_count: usize,
    /// The red subsystems' stable ids (so the rollup can name them).
    pub red_subsystems: Vec<String>,
}

/// Fold a verify board into the compact fleet summary.
///
/// Enablement % is the share of claimed subsystems fully [`ProbeState::Ok`];
/// red is the count of outright [`ProbeState::Failed`] ones. An empty board
/// (non-Surface / no claimed subsystems) is 0% with no reds. Pure.
#[must_use]
pub fn summarize(node: impl Into<String>, board: &VerifyBoard) -> FleetSummary {
    let total = board.rows.len();
    let enabled = board.rows.iter().filter(|r| r.state.is_enabled()).count();
    let red_subsystems: Vec<String> = board
        .rows
        .iter()
        .filter(|r| r.state.is_red())
        .map(|r| r.subsystem.id().to_string())
        .collect();
    // Integer percent, guarding the empty board (0/0 → 0%).
    let enablement_pct = if total == 0 {
        0
    } else {
        u8::try_from(enabled * 100 / total).unwrap_or(100)
    };
    FleetSummary {
        node: node.into(),
        model: board.model.clone(),
        enablement_pct,
        red_count: red_subsystems.len(),
        red_subsystems,
    }
}

fn shared_subsystem(subsystem: Subsystem) -> SurfaceSubsystem {
    match subsystem {
        Subsystem::Touch => SurfaceSubsystem::Touch,
        Subsystem::Pen => SurfaceSubsystem::Pen,
        Subsystem::TypeCover => SurfaceSubsystem::TypeCover,
        Subsystem::Sam => SurfaceSubsystem::Sam,
        Subsystem::RotationAccel => SurfaceSubsystem::RotationAccel,
        Subsystem::Cameras => SurfaceSubsystem::Cameras,
        Subsystem::WifiBt => SurfaceSubsystem::WifiBt,
        Subsystem::S0ix => SurfaceSubsystem::S0ix,
        Subsystem::Fingerprint => SurfaceSubsystem::Fingerprint,
    }
}

fn shared_probe_state(state: ProbeState) -> SurfaceProbeState {
    match state {
        ProbeState::Ok => SurfaceProbeState::Ok,
        ProbeState::Degraded => SurfaceProbeState::Degraded,
        ProbeState::Failed => SurfaceProbeState::Failed,
        ProbeState::NeedsGesture => SurfaceProbeState::NeedsGesture,
    }
}

/// Fold the verifier's private classification values into the one bounded
/// cross-tier state contract. Validation is deliberately producer-side too:
/// hostile or unexpectedly large kernel-provided text is never written to the
/// Bus for downstream consumers to mistake for admitted state.
pub(crate) fn shared_board(
    node: &str,
    detection: &SurfaceDetection,
    board: &VerifyBoard,
    published_at_ms: u64,
) -> Result<SurfaceVerifyBoard, mackes_mesh_types::surface_hardware::SurfaceContractError> {
    let generation = match &detection.model {
        SurfaceModel::Known(device) => device.contract_generation,
        SurfaceModel::UnknownSurface { .. } | SurfaceModel::NotASurface => {
            SurfaceProGeneration::Unsupported
        }
    };
    let availability = board
        .skipped
        .as_ref()
        .map_or(SurfaceAvailability::Fresh, |reason| {
            SurfaceAvailability::Unavailable {
                reason: reason.clone(),
            }
        });
    let publication = SurfacePublication {
        schema_version: SURFACE_HARDWARE_SCHEMA_VERSION,
        node: node.to_string(),
        model: SurfaceModelIdentity {
            product: board.model.clone(),
            generation,
        },
        // The board is a local kernel/device observation. Individual rows say
        // when an operator gesture is still required; this is not an assertion
        // that such a gesture has already occurred.
        source: SurfaceObservationSource::Kernel,
        published_at_ms,
        availability,
    };
    let value = SurfaceVerifyBoard {
        publication,
        skipped: board.skipped.clone(),
        rows: board
            .rows
            .iter()
            .map(|row| SurfaceProbeVerdict {
                subsystem: shared_subsystem(row.subsystem),
                state: shared_probe_state(row.state),
                reason: row.reason.clone(),
            })
            .collect(),
    };
    value.validate()?;
    Ok(value)
}

pub(crate) fn shared_summary(board: &SurfaceVerifyBoard) -> SurfaceFleetSummary {
    let total = board.rows.len();
    let enabled = board
        .rows
        .iter()
        .filter(|row| row.state == SurfaceProbeState::Ok)
        .count();
    let red_subsystems: Vec<_> = board
        .rows
        .iter()
        .filter(|row| row.state == SurfaceProbeState::Failed)
        .map(|row| row.subsystem)
        .collect();
    SurfaceFleetSummary {
        publication: board.publication.clone(),
        enablement_pct: if total == 0 {
            0
        } else {
            u8::try_from(enabled * 100 / total).unwrap_or(100)
        },
        red_count: red_subsystems.len(),
        red_subsystems,
    }
}

// ─────────────────────────── the Bus worker (per-node) ──────────────────────

#[cfg(feature = "async-services")]
pub use worker::{
    board_topic, camera_proof_action_topic, camera_proof_result_topic, summary_topic,
    SurfaceVerifyWorker,
};

#[cfg(feature = "async-services")]
mod worker {
    //! The per-node `surface_verify` Bus worker (a *leader-of-self* worker:
    //! it probes only its own hardware, never a remote node). Each tick it
    //! runs [`super::run_verify`] against the hardware-backed
    //! [`super::LiveSurfaceProbes`], publishes the full board to
    //! [`board_topic`] (SURFACE-6's Test tab), and the compact
    //! [`super::FleetSummary`] to [`summary_topic`] (the fleet rollup, lock
    //! #7). On a non-Surface node it idles (never touches the Bus).

    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use mackes_mesh_types::surface_hardware::{
        SurfaceCameraProofOutcome, SurfaceCameraProofRefusal, SurfaceCameraProofRequest,
        SurfaceCameraProofResult, SurfaceCameraProofUnavailable, SurfaceModelIdentity,
        SurfaceProGeneration, SURFACE_CAMERA_PROOF_ARM_TOKEN, SURFACE_HARDWARE_SCHEMA_VERSION,
    };
    use mde_bus::hooks::config::Priority;
    use mde_bus::persist::Persist;

    use super::{
        run_verify, shared_board, shared_summary, CameraFunctionalProofProvider,
        LiveCameraFunctionalProofProvider, LiveSurfaceProbes,
    };
    use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};
    use crate::surface::{detect, SurfaceDetection, SurfaceModel};
    use crate::workers::{ShutdownToken, Worker};

    /// Re-verify cadence — the board is fleet-visibility, not hot-path, so a
    /// modest tick keeps the rollup fresh without churn.
    pub const POLL: Duration = Duration::from_secs(30);

    /// Camera actions poll with deterministic headroom inside the shared 30s
    /// action lifetime; verification-board enumeration stays on [`POLL`].
    pub const CAMERA_ACTION_POLL: Duration = Duration::from_secs(1);

    /// Closed capability verb for one non-retaining camera functional proof.
    pub const CAMERA_PROOF_ACTION_AUTH_VERB: &str = "surface-camera-functional-proof";

    /// Stable capability target. The exact body additionally binds generation,
    /// request identity, timestamps, and both arming fields.
    pub const CAMERA_PROOF_ACTION_AUTH_TARGET: &str = "one-frame-discard";

    /// The per-node lane the full tri-state board lands on (Test tab).
    #[must_use]
    pub fn board_topic(node: &str) -> String {
        format!("state/hardware/surface/{node}/probes")
    }

    /// The per-node lane the compact fleet summary lands on (the rollup).
    #[must_use]
    pub fn summary_topic(node: &str) -> String {
        format!("state/hardware/surface/{node}")
    }

    /// Per-node request lane for the separately armed camera proof.
    #[must_use]
    pub fn camera_proof_action_topic(node: &str) -> String {
        format!("action/hardware/surface/{node}/camera-proof")
    }

    /// Per-node privacy-safe result lane for camera proof outcomes.
    #[must_use]
    pub fn camera_proof_result_topic(node: &str) -> String {
        format!("state/hardware/surface/{node}/camera-proof")
    }

    /// The per-node `surface_verify` worker.
    pub struct SurfaceVerifyWorker {
        node_id: String,
        detection: SurfaceDetection,
        bus_root: Option<PathBuf>,
        poll: Duration,
        camera_action_cursor: Option<String>,
        authorizer: Arc<ActionAuthorizer>,
        camera_provider: Arc<dyn CameraFunctionalProofProvider>,
    }

    impl SurfaceVerifyWorker {
        /// Build the worker for `node_id`, detecting this host's Surface
        /// identity now (SURFACE-2's [`detect`]).
        #[must_use]
        pub fn new(node_id: String) -> Self {
            Self {
                node_id,
                detection: detect(),
                bus_root: default_bus_root(),
                poll: POLL,
                camera_action_cursor: None,
                authorizer: Arc::new(ActionAuthorizer::production()),
                camera_provider: Arc::new(LiveCameraFunctionalProofProvider),
            }
        }

        /// Test constructor: an explicit detection + bus root, no real /sys.
        #[cfg(test)]
        #[must_use]
        pub(crate) fn with_parts(
            node_id: String,
            detection: SurfaceDetection,
            bus_root: PathBuf,
        ) -> Self {
            Self {
                node_id,
                detection,
                bus_root: Some(bus_root),
                poll: POLL,
                camera_action_cursor: None,
                authorizer: Arc::new(ActionAuthorizer::production()),
                camera_provider: Arc::new(LiveCameraFunctionalProofProvider),
            }
        }

        /// Focused action test constructor with injected authorization and
        /// provider seams. Production always installs the root verifier and
        /// fixed libcamera provider.
        #[cfg(test)]
        pub(crate) fn with_camera_parts(
            node_id: String,
            detection: SurfaceDetection,
            bus_root: PathBuf,
            authorizer: Arc<ActionAuthorizer>,
            camera_provider: Arc<dyn CameraFunctionalProofProvider>,
        ) -> Self {
            Self {
                node_id,
                detection,
                bus_root: Some(bus_root),
                poll: POLL,
                camera_action_cursor: None,
                authorizer,
                camera_provider,
            }
        }

        /// Probe once and publish the board + the compact summary. Pulled out
        /// so a test drives it against a temp Bus without the run loop/clock.
        fn probe_once(&self, persist: &Persist) {
            let board = run_verify(&LiveSurfaceProbes, &self.detection);
            let published_at_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
                .unwrap_or(1);
            let Ok(board) = shared_board(&self.node_id, &self.detection, &board, published_at_ms)
            else {
                tracing::warn!(
                    target: "mackesd::surface_verify",
                    "refusing invalid Surface verification publication"
                );
                return;
            };
            publish(persist, &board_topic(&self.node_id), &board);
            let summary = shared_summary(&board);
            debug_assert!(summary.validate().is_ok());
            publish(persist, &summary_topic(&self.node_id), &summary);
        }

        fn admitted_camera_model(&self) -> Option<SurfaceModelIdentity> {
            let SurfaceModel::Known(device) = &self.detection.model else {
                return None;
            };
            match (&*device.product, device.contract_generation) {
                ("Surface Pro 5", SurfaceProGeneration::Pro5)
                | ("Surface Pro 6", SurfaceProGeneration::Pro6) => Some(SurfaceModelIdentity {
                    product: device.product.clone(),
                    generation: device.contract_generation,
                }),
                _ => None,
            }
        }

        fn camera_result(
            &self,
            request_id: &str,
            model: Option<SurfaceModelIdentity>,
            outcome: SurfaceCameraProofOutcome,
        ) -> SurfaceCameraProofResult {
            SurfaceCameraProofResult {
                schema_version: SURFACE_HARDWARE_SCHEMA_VERSION,
                node: self.node_id.clone(),
                request_id: if request_id.is_empty() {
                    "unadmitted".to_string()
                } else {
                    request_id.to_string()
                },
                model,
                completed_at_ms: wall_now_ms(),
                outcome,
            }
        }

        fn camera_request(&self, body: Option<&str>) -> SurfaceCameraProofResult {
            let Some(model) = self.admitted_camera_model() else {
                return self.camera_result(
                    "unadmitted",
                    None,
                    SurfaceCameraProofOutcome::Unavailable(
                        SurfaceCameraProofUnavailable::UnsupportedModel,
                    ),
                );
            };
            let Some(body) = body else {
                return self.camera_result(
                    "unadmitted",
                    Some(model),
                    SurfaceCameraProofOutcome::Refused(SurfaceCameraProofRefusal::Contract),
                );
            };
            let request = match SurfaceCameraProofRequest::from_json_at(
                body.as_bytes(),
                &self.node_id,
                wall_now_ms(),
            ) {
                Ok(request) => request,
                Err(_) => {
                    return self.camera_result(
                        "unadmitted",
                        Some(model),
                        SurfaceCameraProofOutcome::Refused(SurfaceCameraProofRefusal::Contract),
                    );
                }
            };
            if request.generation != model.generation {
                return self.camera_result(
                    &request.header.request_id,
                    Some(model),
                    SurfaceCameraProofOutcome::Refused(
                        SurfaceCameraProofRefusal::GenerationMismatch,
                    ),
                );
            }
            let context = MutationContext {
                verb: CAMERA_PROOF_ACTION_AUTH_VERB,
                node: &self.node_id,
                target: CAMERA_PROOF_ACTION_AUTH_TARGET,
            };
            if self.authorizer.authorize(body, context).is_err() {
                tracing::warn!(
                    target: "mackesd::surface_verify",
                    node = %self.node_id,
                    "refused unauthorized camera functional proof"
                );
                return self.camera_result(
                    &request.header.request_id,
                    Some(model),
                    SurfaceCameraProofOutcome::Refused(SurfaceCameraProofRefusal::Authorization),
                );
            }
            if request.arm_token.as_deref() != Some(SURFACE_CAMERA_PROOF_ARM_TOKEN) {
                return self.camera_result(
                    &request.header.request_id,
                    Some(model),
                    SurfaceCameraProofOutcome::Refused(SurfaceCameraProofRefusal::OperatorArm),
                );
            }
            self.camera_result(
                &request.header.request_id,
                Some(model),
                self.camera_provider.prove_one_frame(),
            )
        }

        fn poll_camera_actions(&mut self, persist: &Persist) {
            let topic = camera_proof_action_topic(&self.node_id);
            let Ok(messages) = persist.list_since(&topic, self.camera_action_cursor.as_deref())
            else {
                return;
            };
            for message in messages {
                self.camera_action_cursor = Some(message.ulid.clone());
                let result = self.camera_request(message.body.as_deref());
                debug_assert!(result.validate().is_ok());
                publish(persist, &camera_proof_result_topic(&self.node_id), &result);
            }
        }
    }

    /// Publish a serializable payload to `topic` (best-effort; a failed write
    /// is logged, not fatal).
    fn publish<T: serde::Serialize>(persist: &Persist, topic: &str, payload: &T) {
        let Ok(body) = serde_json::to_string(payload) else {
            return;
        };
        if let Err(e) = persist.write(topic, Priority::Default, None, Some(&body)) {
            tracing::debug!(
                target: "mackesd::surface_verify",
                topic,
                error = %e,
                "verify publish failed"
            );
        }
    }

    /// The default Bus root (same shape the other bus workers use).
    fn default_bus_root() -> Option<PathBuf> {
        mde_bus::default_data_dir()
    }

    fn wall_now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
            .unwrap_or(1)
    }

    #[async_trait::async_trait]
    impl Worker for SurfaceVerifyWorker {
        fn name(&self) -> &'static str {
            "surface_verify"
        }

        async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
            // Non-Surface node: the card never appears, so the worker idles
            // (it never touches the Bus) rather than publish an empty board.
            if !self.detection.model.is_surface() {
                tracing::debug!(
                    target: "mackesd::surface_verify",
                    "not a Surface; worker idle"
                );
                shutdown.wait().await;
                return Ok(());
            }
            let Some(root) = self.bus_root.clone() else {
                tracing::debug!(target: "mackesd::surface_verify", "no bus root; worker idle");
                shutdown.wait().await;
                return Ok(());
            };
            let mut next_probe = Instant::now();
            loop {
                match Persist::open(root.clone()) {
                    Ok(persist) => {
                        let now = Instant::now();
                        if now >= next_probe {
                            self.probe_once(&persist);
                            next_probe = now + self.poll;
                        }
                        self.poll_camera_actions(&persist);
                    }
                    Err(e) => tracing::debug!(
                        target: "mackesd::surface_verify",
                        error = %e,
                        "bus open failed"
                    ),
                }
                let until_probe = next_probe.saturating_duration_since(Instant::now());
                let delay = if until_probe.is_zero() {
                    CAMERA_ACTION_POLL
                } else {
                    CAMERA_ACTION_POLL.min(until_probe)
                };
                tokio::select! {
                    () = tokio::time::sleep(delay) => {}
                    () = shutdown.wait() => return Ok(()),
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        use super::*;
        use crate::ipc::action_auth::{authorize_test_body, ActionAuthorizer, MutationContext};
        use crate::surface::{identify, DmiInfo, MS_VENDOR};
        use mackes_mesh_types::surface_hardware::{
            SurfaceActionHeader, SurfaceCameraProofFailure, SurfaceCameraProofOutcome,
            SurfaceCameraProofRequest, SurfaceCameraProofResult, SurfaceCameraProofUnavailable,
            SurfaceFleetSummary, SurfaceProGeneration, SurfaceVerifyBoard,
            SURFACE_CAMERA_PROOF_ARM_TOKEN,
        };

        const AUTH_KEY: &[u8] = b"surface-camera-functional-proof-test-key";
        const AUTH_NOW: i64 = 1_800_000_000_000;

        fn detection(product: &str) -> SurfaceDetection {
            let mut dmi = DmiInfo {
                sys_vendor: MS_VENDOR.to_string(),
                product_name: product.to_string(),
                product_sku: String::new(),
                ..Default::default()
            };
            if product == "Surface Pro 5" {
                dmi.product_name = "Surface Pro".to_string();
                dmi.product_sku = "Surface_Pro_1796".to_string();
            }
            SurfaceDetection {
                model: identify(&dmi),
                dmi,
            }
        }

        struct FakeCameraProvider {
            calls: Arc<AtomicUsize>,
            outcome: SurfaceCameraProofOutcome,
        }

        impl CameraFunctionalProofProvider for FakeCameraProvider {
            fn prove_one_frame(&self) -> SurfaceCameraProofOutcome {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.outcome
            }
        }

        fn camera_worker(
            node: &str,
            detection: SurfaceDetection,
            root: &std::path::Path,
            calls: Arc<AtomicUsize>,
            outcome: SurfaceCameraProofOutcome,
        ) -> SurfaceVerifyWorker {
            SurfaceVerifyWorker::with_camera_parts(
                node.to_string(),
                detection,
                root.to_path_buf(),
                Arc::new(ActionAuthorizer::for_test(
                    AUTH_KEY,
                    root.join("auth"),
                    AUTH_NOW,
                )),
                Arc::new(FakeCameraProvider { calls, outcome }),
            )
        }

        fn signed_camera_request(
            node: &str,
            generation: SurfaceProGeneration,
            arm: Option<&str>,
            nonce: &str,
        ) -> String {
            let unsigned = serde_json::to_string(&SurfaceCameraProofRequest {
                header: SurfaceActionHeader {
                    schema_version: SURFACE_HARDWARE_SCHEMA_VERSION,
                    node: node.to_string(),
                    request_id: nonce.to_string(),
                    issued_at_ms: wall_now_ms(),
                    armed_token: None,
                },
                generation,
                arm_token: arm.map(str::to_string),
            })
            .unwrap();
            authorize_test_body(
                AUTH_KEY,
                &unsigned,
                MutationContext {
                    verb: CAMERA_PROOF_ACTION_AUTH_VERB,
                    node,
                    target: CAMERA_PROOF_ACTION_AUTH_TARGET,
                },
                nonce,
                AUTH_NOW + 30_000,
            )
        }

        #[test]
        fn default_bus_root_honors_mde_bus_root() {
            let root = tempfile::tempdir().expect("tempdir");
            let expected = root.path().to_path_buf();
            let previous = std::env::var_os("MDE_BUS_ROOT");
            let got = {
                std::env::set_var("MDE_BUS_ROOT", &expected);
                let got = SurfaceVerifyWorker::new("node-a".into()).bus_root;
                match previous {
                    Some(value) => std::env::set_var("MDE_BUS_ROOT", value),
                    None => std::env::remove_var("MDE_BUS_ROOT"),
                }
                got
            };

            assert_eq!(got, Some(expected));
        }

        #[test]
        fn camera_action_poll_has_deterministic_freshness_headroom() {
            assert!(
                CAMERA_ACTION_POLL.as_millis()
                    < u128::from(
                        mackes_mesh_types::surface_hardware::MAX_SURFACE_ACTION_AGE_MS / 3
                    )
            );
            assert_eq!(POLL, Duration::from_secs(30));
        }

        #[test]
        fn publishes_board_and_summary_for_a_surface() {
            let dir = tempfile::tempdir().expect("tempdir");
            let persist = Persist::open(dir.path().to_path_buf()).expect("open bus");
            let w = SurfaceVerifyWorker::with_parts(
                "node-a".into(),
                detection("Surface Pro 6"),
                dir.path().to_path_buf(),
            );

            w.probe_once(&persist);

            let boards = persist
                .list_since(&board_topic("node-a"), None)
                .expect("list boards");
            assert_eq!(boards.len(), 1, "one board published");
            let board =
                SurfaceVerifyBoard::from_json(boards[0].body.as_deref().unwrap().as_bytes())
                    .unwrap();
            assert_eq!(board.publication.model.product, "Surface Pro 6");
            assert_eq!(
                board.publication.model.generation,
                SurfaceProGeneration::Pro6
            );
            assert!(!board.rows.is_empty(), "the Pro claims subsystems");

            let summaries = persist
                .list_since(&summary_topic("node-a"), None)
                .expect("list summaries");
            assert_eq!(summaries.len(), 1, "one summary published");
            let summary =
                SurfaceFleetSummary::from_json(summaries[0].body.as_deref().unwrap().as_bytes())
                    .unwrap();
            assert_eq!(summary.publication.node, "node-a");
            assert_eq!(summary.publication.model.product, "Surface Pro 6");
            // This non-Surface farm host exposes none of the Pro hardware,
            // so enablement is honestly 0% (never a faked green).
            assert_eq!(summary.enablement_pct, 0);
        }

        #[test]
        fn authorized_armed_pro5_and_pro6_reach_provider_once_and_publish_closed_result() {
            for (product, generation) in [
                ("Surface Pro 5", SurfaceProGeneration::Pro5),
                ("Surface Pro 6", SurfaceProGeneration::Pro6),
            ] {
                let dir = tempfile::tempdir().unwrap();
                let persist = Persist::open(dir.path().to_path_buf()).unwrap();
                let calls = Arc::new(AtomicUsize::new(0));
                let mut worker = camera_worker(
                    "surface",
                    detection(product),
                    dir.path(),
                    Arc::clone(&calls),
                    SurfaceCameraProofOutcome::Passed,
                );
                let body = signed_camera_request(
                    "surface",
                    generation,
                    Some(SURFACE_CAMERA_PROOF_ARM_TOKEN),
                    match generation {
                        SurfaceProGeneration::Pro5 => "camera-proof-pro5",
                        SurfaceProGeneration::Pro6 => "camera-proof-pro6",
                        SurfaceProGeneration::Unsupported => unreachable!(),
                    },
                );
                persist
                    .write(
                        &camera_proof_action_topic("surface"),
                        Priority::Default,
                        None,
                        Some(&body),
                    )
                    .unwrap();
                worker.poll_camera_actions(&persist);
                assert_eq!(calls.load(Ordering::SeqCst), 1);
                let results = persist
                    .list_since(&camera_proof_result_topic("surface"), None)
                    .unwrap();
                assert_eq!(results.len(), 1);
                let raw = results[0].body.as_deref().unwrap();
                assert!(!raw.contains("/dev/video"));
                assert!(!raw.contains("camera0"));
                assert!(!raw.contains("frame"));
                let result = SurfaceCameraProofResult::from_json(raw.as_bytes()).unwrap();
                assert_eq!(result.outcome, SurfaceCameraProofOutcome::Passed);
                assert_eq!(result.model.unwrap().generation, generation);
            }
        }

        #[test]
        fn missing_auth_wrong_arm_generation_replay_and_unsupported_never_reach_provider() {
            let dir = tempfile::tempdir().unwrap();
            let calls = Arc::new(AtomicUsize::new(0));
            let worker = camera_worker(
                "surface",
                detection("Surface Pro 6"),
                dir.path(),
                Arc::clone(&calls),
                SurfaceCameraProofOutcome::Failed(SurfaceCameraProofFailure::CaptureFailed),
            );

            let unsigned = serde_json::to_string(&SurfaceCameraProofRequest {
                header: SurfaceActionHeader {
                    schema_version: SURFACE_HARDWARE_SCHEMA_VERSION,
                    node: "surface".into(),
                    request_id: "unsigned-camera-proof".into(),
                    issued_at_ms: wall_now_ms(),
                    armed_token: None,
                },
                generation: SurfaceProGeneration::Pro6,
                arm_token: Some(SURFACE_CAMERA_PROOF_ARM_TOKEN.into()),
            })
            .unwrap();
            assert!(matches!(
                worker.camera_request(Some(&unsigned)).outcome,
                SurfaceCameraProofOutcome::Refused(SurfaceCameraProofRefusal::Authorization)
            ));

            let wrong_generation = signed_camera_request(
                "surface",
                SurfaceProGeneration::Pro5,
                Some(SURFACE_CAMERA_PROOF_ARM_TOKEN),
                "wrong-camera-generation",
            );
            assert!(matches!(
                worker.camera_request(Some(&wrong_generation)).outcome,
                SurfaceCameraProofOutcome::Refused(SurfaceCameraProofRefusal::GenerationMismatch)
            ));

            let wrong_arm = signed_camera_request(
                "surface",
                SurfaceProGeneration::Pro6,
                Some("CAPTURE CAMERA"),
                "wrong-camera-arm",
            );
            assert!(matches!(
                worker.camera_request(Some(&wrong_arm)).outcome,
                SurfaceCameraProofOutcome::Refused(SurfaceCameraProofRefusal::OperatorArm)
            ));

            let once = signed_camera_request(
                "surface",
                SurfaceProGeneration::Pro6,
                Some(SURFACE_CAMERA_PROOF_ARM_TOKEN),
                "camera-replay-once",
            );
            assert!(matches!(
                worker.camera_request(Some(&once)).outcome,
                SurfaceCameraProofOutcome::Failed(SurfaceCameraProofFailure::CaptureFailed)
            ));
            assert!(matches!(
                worker.camera_request(Some(&once)).outcome,
                SurfaceCameraProofOutcome::Refused(SurfaceCameraProofRefusal::Authorization)
            ));
            assert_eq!(calls.load(Ordering::SeqCst), 1);

            let unsupported_calls = Arc::new(AtomicUsize::new(0));
            let unsupported = camera_worker(
                "surface",
                detection("Surface Pro 8"),
                dir.path(),
                Arc::clone(&unsupported_calls),
                SurfaceCameraProofOutcome::Passed,
            );
            assert_eq!(
                unsupported.camera_request(Some(&once)).outcome,
                SurfaceCameraProofOutcome::Unavailable(
                    SurfaceCameraProofUnavailable::UnsupportedModel
                )
            );
            assert_eq!(unsupported_calls.load(Ordering::SeqCst), 0);
        }
    }
}

// ─────────────────────────────── tests ──────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::{identify, DmiInfo, MS_VENDOR};

    /// A fully scripted fake seam so the folds + board run green without a
    /// machine. Each field drives the matching probe's reading.
    #[derive(Clone)]
    struct FakeProbes {
        touch: Result<InputPresence, ProbeError>,
        pen: Result<PenReading, ProbeError>,
        type_cover: Result<InputPresence, ProbeError>,
        sam: Result<SamReading, ProbeError>,
        accel: Result<AccelReading, ProbeError>,
        camera: Result<CameraReading, ProbeError>,
        wifi_bt: Result<WifiBtReading, ProbeError>,
        s0ix: Result<S0ixReading, ProbeError>,
        fingerprint: Result<FingerprintReading, ProbeError>,
    }

    impl Default for FakeProbes {
        /// A healthy Surface: everything green, gestures satisfied.
        fn default() -> Self {
            Self {
                touch: Ok(InputPresence {
                    present: true,
                    name: "IPTS Touch".into(),
                }),
                pen: Ok(PenReading {
                    digitizer_present: true,
                    pressure_seen: true,
                    tilt_seen: true,
                }),
                type_cover: Ok(InputPresence {
                    present: true,
                    name: "Surface Type Cover".into(),
                }),
                sam: Ok(SamReading {
                    battery_readable: true,
                    thermal_readable: true,
                }),
                accel: Ok(AccelReading {
                    vector: Some([0.0, 0.0, 1000.0]),
                }),
                camera: Ok(CameraReading {
                    device_present: true,
                    pipeline_ready: true,
                }),
                wifi_bt: Ok(WifiBtReading {
                    wifi_up: true,
                    bt_up: true,
                }),
                s0ix: Ok(S0ixReading {
                    residency_counter: Some(42),
                    advanced: Some(true),
                }),
                fingerprint: Ok(FingerprintReading {
                    device_present: true,
                    stack_ready: true,
                }),
            }
        }
    }

    impl SurfaceProbes for FakeProbes {
        fn probe_touch(&self) -> Result<InputPresence, ProbeError> {
            self.touch.clone()
        }
        fn probe_pen(&self) -> Result<PenReading, ProbeError> {
            self.pen.clone()
        }
        fn probe_type_cover(&self) -> Result<InputPresence, ProbeError> {
            self.type_cover.clone()
        }
        fn probe_sam(&self) -> Result<SamReading, ProbeError> {
            self.sam.clone()
        }
        fn probe_accelerometer(&self) -> Result<AccelReading, ProbeError> {
            self.accel.clone()
        }
        fn probe_camera(&self) -> Result<CameraReading, ProbeError> {
            self.camera.clone()
        }
        fn probe_wifi_bt(&self) -> Result<WifiBtReading, ProbeError> {
            self.wifi_bt.clone()
        }
        fn probe_s0ix(&self) -> Result<S0ixReading, ProbeError> {
            self.s0ix.clone()
        }
        fn probe_fingerprint(&self) -> Result<FingerprintReading, ProbeError> {
            self.fingerprint.clone()
        }
    }

    fn detect_of(product: &str) -> SurfaceDetection {
        let (product_name, product_sku) = if product == "Surface Pro 5" {
            ("Surface Pro", "Surface_Pro_1796")
        } else {
            (product, "")
        };
        let dmi = DmiInfo {
            sys_vendor: MS_VENDOR.to_string(),
            product_name: product_name.to_string(),
            product_sku: product_sku.to_string(),
            ..Default::default()
        };
        SurfaceDetection {
            model: identify(&dmi),
            dmi,
        }
    }

    fn state_of(board: &VerifyBoard, s: Subsystem) -> ProbeState {
        board
            .rows
            .iter()
            .find(|r| r.subsystem == s)
            .expect("subsystem row present on the board")
            .state
    }

    // ── the classification folds (each branch) ──────────────────────────────

    #[test]
    fn touch_present_is_ok_absent_is_failed() {
        assert_eq!(
            classify_touch(Ok(InputPresence {
                present: true,
                name: "IPTS".into()
            }))
            .state,
            ProbeState::Ok
        );
        assert_eq!(
            classify_touch(Ok(InputPresence {
                present: false,
                name: String::new()
            }))
            .state,
            ProbeState::Failed
        );
    }

    #[test]
    fn pen_folds_all_four_states() {
        let ok = classify_pen(Ok(PenReading {
            digitizer_present: true,
            pressure_seen: true,
            tilt_seen: true,
        }));
        assert_eq!(ok.state, ProbeState::Ok);

        let degraded = classify_pen(Ok(PenReading {
            digitizer_present: true,
            pressure_seen: true,
            tilt_seen: false,
        }));
        assert_eq!(degraded.state, ProbeState::Degraded);

        // Enumerated but no samples yet → prompt a gesture, don't fault.
        let gesture = classify_pen(Ok(PenReading {
            digitizer_present: true,
            pressure_seen: false,
            tilt_seen: false,
        }));
        assert_eq!(gesture.state, ProbeState::NeedsGesture);
        assert!(gesture.reason.contains("pen"));

        let failed = classify_pen(Ok(PenReading {
            digitizer_present: false,
            pressure_seen: false,
            tilt_seen: false,
        }));
        assert_eq!(failed.state, ProbeState::Failed);
    }

    #[test]
    fn sam_needs_both_battery_and_thermal_for_green() {
        assert_eq!(
            classify_sam(Ok(SamReading {
                battery_readable: true,
                thermal_readable: true
            }))
            .state,
            ProbeState::Ok
        );
        assert_eq!(
            classify_sam(Ok(SamReading {
                battery_readable: true,
                thermal_readable: false
            }))
            .state,
            ProbeState::Degraded
        );
        assert_eq!(
            classify_sam(Ok(SamReading {
                battery_readable: false,
                thermal_readable: false
            }))
            .state,
            ProbeState::Failed
        );
    }

    #[test]
    fn accelerometer_needs_a_plausible_vector() {
        assert_eq!(
            classify_accelerometer(Ok(AccelReading {
                vector: Some([0.0, 0.0, 981.0])
            }))
            .state,
            ProbeState::Ok
        );
        assert_eq!(
            classify_accelerometer(Ok(AccelReading {
                vector: Some([0.0, 0.0, 0.0])
            }))
            .state,
            ProbeState::Degraded
        );
        assert_eq!(
            classify_accelerometer(Ok(AccelReading { vector: None })).state,
            ProbeState::Failed
        );
    }

    #[test]
    fn wifi_bt_one_radio_down_is_degraded() {
        assert_eq!(
            classify_wifi_bt(Ok(WifiBtReading {
                wifi_up: true,
                bt_up: true
            }))
            .state,
            ProbeState::Ok
        );
        assert_eq!(
            classify_wifi_bt(Ok(WifiBtReading {
                wifi_up: true,
                bt_up: false
            }))
            .state,
            ProbeState::Degraded
        );
        assert_eq!(
            classify_wifi_bt(Ok(WifiBtReading {
                wifi_up: false,
                bt_up: false
            }))
            .state,
            ProbeState::Failed
        );
    }

    #[test]
    fn s0ix_folds_gesture_advance_and_failure() {
        // Counter present, no suspend measured → gesture.
        assert_eq!(
            classify_s0ix(Ok(S0ixReading {
                residency_counter: Some(1),
                advanced: None
            }))
            .state,
            ProbeState::NeedsGesture
        );
        // Advanced → green.
        assert_eq!(
            classify_s0ix(Ok(S0ixReading {
                residency_counter: Some(1),
                advanced: Some(true)
            }))
            .state,
            ProbeState::Ok
        );
        // Suspended but did not advance → honest red.
        assert_eq!(
            classify_s0ix(Ok(S0ixReading {
                residency_counter: Some(1),
                advanced: Some(false)
            }))
            .state,
            ProbeState::Failed
        );
        // No counter at all → red.
        assert_eq!(
            classify_s0ix(Ok(S0ixReading {
                residency_counter: None,
                advanced: None
            }))
            .state,
            ProbeState::Failed
        );
    }

    #[test]
    fn fingerprint_present_but_stack_unavailable_is_degraded() {
        assert_eq!(
            classify_fingerprint(Ok(FingerprintReading {
                device_present: true,
                stack_ready: true
            }))
            .state,
            ProbeState::Ok
        );
        assert_eq!(
            classify_fingerprint(Ok(FingerprintReading {
                device_present: true,
                stack_ready: false
            }))
            .state,
            ProbeState::Degraded
        );
        assert_eq!(
            classify_fingerprint(Ok(FingerprintReading {
                device_present: false,
                stack_ready: false
            }))
            .state,
            ProbeState::Failed
        );
    }

    #[test]
    fn libcamera_parser_requires_its_header_and_numbered_camera_rows() {
        let listed = "[0:00:00.000] INFO Camera camera_manager.cpp\n\
                      Available cameras:\n\
                      0: 'Front Camera' (/base/ipu3/camera0)\n\
                      1: 'Rear Camera' (/base/ipu3/camera1)\n";
        assert_eq!(parse_libcamera_list(listed), Ok(true));
        assert_eq!(parse_libcamera_list("Available cameras:\n"), Ok(false));
        assert!(parse_libcamera_list("0: camera-shaped noise\n").is_err());
    }

    #[test]
    fn libcamera_capability_probe_is_fixed_bounded_and_never_captures() {
        assert_eq!(LiveSurfaceProbes::LIBCAMERA_CAM, "/usr/bin/cam");
        assert_eq!(LiveSurfaceProbes::LIBCAMERA_ARGS, ["--list"]);
        assert_eq!(LiveSurfaceProbes::LIBCAMERA_TIMEOUT, Duration::from_secs(5));
        assert_eq!(LiveSurfaceProbes::LIBCAMERA_OUTPUT_LIMIT, 64 * 1024);

        let verdict = classify_camera(Ok(CameraReading {
            device_present: true,
            pipeline_ready: true,
        }));
        assert_eq!(verdict.state, ProbeState::Ok);
        assert!(verdict.reason.contains("no frame captured"));
    }

    #[test]
    fn functional_camera_provider_is_one_frame_fixed_discarding_and_bounded() {
        assert_eq!(LiveCameraFunctionalProofProvider::PROGRAM, "/usr/bin/cam");
        assert_eq!(
            LiveCameraFunctionalProofProvider::ARGS,
            ["--camera=1", "--capture=1", "--file=/dev/null"]
        );
        assert_eq!(
            LiveCameraFunctionalProofProvider::TIMEOUT,
            Duration::from_secs(8)
        );
        assert!(LiveCameraFunctionalProofProvider::ARGS
            .iter()
            .all(|argument| !argument.contains("shell")));
    }

    #[test]
    fn libcamera_parser_rejects_hostile_or_ambiguous_rows() {
        for hostile in [
            "Available cameras:\n0: no-device-path\n",
            "Available cameras:\n0: Camera (/one)\n0: Camera (/two)\n",
            "Available cameras:\n16: Camera (/path)\n",
            "Available cameras:\ncamera-shaped noise\n",
            "Available cameras:\nAvailable cameras:\n",
            "Available cameras:\n0: Camera (/path)\0\n",
        ] {
            assert!(
                parse_libcamera_list(hostile).is_err(),
                "accepted {hostile:?}"
            );
        }
    }

    #[test]
    fn fprintd_inventory_is_fixed_read_only_bounded_and_strictly_parsed() {
        assert_eq!(LiveSurfaceProbes::BUSCTL, "/usr/bin/busctl");
        assert_eq!(
            LiveSurfaceProbes::FPRINT_MANAGER_ARGS,
            [
                "--system",
                "--timeout=5",
                "call",
                "net.reactivated.Fprint",
                "/net/reactivated/Fprint/Manager",
                "net.reactivated.Fprint.Manager",
                "GetDevices",
            ]
        );
        assert_eq!(LiveSurfaceProbes::FPRINT_TIMEOUT, Duration::from_secs(5));
        assert_eq!(LiveSurfaceProbes::FPRINT_OUTPUT_LIMIT, 16 * 1024);
        assert_eq!(parse_fprintd_devices("ao 0\n"), Ok(Vec::new()));
        assert_eq!(
            parse_fprintd_devices(
                "ao 2 \"/net/reactivated/Fprint/Device/0\" \"/net/reactivated/Fprint/Device/reader_1\"\n"
            ),
            Ok(vec![
                "/net/reactivated/Fprint/Device/0".to_string(),
                "/net/reactivated/Fprint/Device/reader_1".to_string(),
            ])
        );
    }

    #[test]
    fn fprintd_parser_rejects_hostile_or_inconsistent_inventory() {
        for hostile in [
            "ao 1",
            "ao 0 \"/net/reactivated/Fprint/Device/0\"",
            "ao 1 \"/not-fprintd/Device/0\"",
            "ao 1 \"/net/reactivated/Fprint/Device/../Manager\"",
            "ao 1 \"/net/reactivated/Fprint/Device/0\" trailing",
            "ao 17 \"/net/reactivated/Fprint/Device/0\"",
            "ao 01 \"/net/reactivated/Fprint/Device/0\"",
            "ao 1 \"/net/reactivated/Fprint/Device/0\"\nextra\n",
        ] {
            assert!(
                parse_fprintd_devices(hostile).is_err(),
                "accepted {hostile:?}"
            );
        }
    }

    #[test]
    fn bounded_reader_reports_and_truncates_oversize_output() {
        let (bytes, overflow) = read_bounded(&b"12345"[..], 4).expect("bounded read");
        assert_eq!(bytes, b"1234");
        assert!(overflow);
    }

    #[test]
    fn a_gated_probe_is_honestly_red_not_fake_green() {
        let v = classify_camera(Err(ProbeError::IntegrationGated {
            probe: "camera frame capture".into(),
        }));
        assert_eq!(v.state, ProbeState::Failed);
        assert!(v.reason.contains("integration-gated"));
    }

    // ── the profile-gated probe selection ───────────────────────────────────

    #[test]
    fn a_laptop_board_omits_the_type_cover_row() {
        // A clamshell Laptop has no detachable Type Cover → that row must not
        // appear (verify neither probes nor faults it).
        let board = run_verify(&FakeProbes::default(), &detect_of("Surface Laptop 3"));
        assert!(!board
            .rows
            .iter()
            .any(|r| r.subsystem == Subsystem::TypeCover));
        assert!(!board
            .rows
            .iter()
            .any(|r| r.subsystem == Subsystem::RotationAccel));
        // But it DOES claim + probe the fingerprint reader.
        assert_eq!(state_of(&board, Subsystem::Fingerprint), ProbeState::Ok);
    }

    #[test]
    fn a_pro_board_probes_the_full_2in1_matrix() {
        let board = run_verify(&FakeProbes::default(), &detect_of("Surface Pro 8"));
        for s in [
            Subsystem::Touch,
            Subsystem::Pen,
            Subsystem::TypeCover,
            Subsystem::Sam,
            Subsystem::RotationAccel,
            Subsystem::Cameras,
            Subsystem::WifiBt,
            Subsystem::S0ix,
        ] {
            assert_eq!(state_of(&board, s), ProbeState::Ok, "{s:?} should be green");
        }
        // The Pro has IR-face, not a fingerprint reader — not claimed/probed.
        assert!(!board
            .rows
            .iter()
            .any(|r| r.subsystem == Subsystem::Fingerprint));
    }

    #[test]
    fn pro_5_and_6_keep_exact_model_identity_and_camera_evidence() {
        for product in ["Surface Pro 5", "Surface Pro 6"] {
            let board = run_verify(&FakeProbes::default(), &detect_of(product));
            assert_eq!(board.model, product);
            assert!(board.skipped.is_none());
            assert_eq!(state_of(&board, Subsystem::Cameras), ProbeState::Ok);
            assert!(board
                .rows
                .iter()
                .any(|row| row.subsystem == Subsystem::TypeCover));
        }
    }

    #[test]
    fn shared_publication_carries_pro_5_and_6_identity_source_and_freshness() {
        for (product, generation) in [
            ("Surface Pro 5", SurfaceProGeneration::Pro5),
            ("Surface Pro 6", SurfaceProGeneration::Pro6),
        ] {
            let detection = detect_of(product);
            let private = run_verify(&FakeProbes::default(), &detection);
            let board = shared_board("surface-seat", &detection, &private, 42)
                .expect("healthy fixture admits to shared contract");
            assert_eq!(board.publication.node, "surface-seat");
            assert_eq!(board.publication.model.product, product);
            assert_eq!(board.publication.model.generation, generation);
            assert_eq!(board.publication.source, SurfaceObservationSource::Kernel);
            assert_eq!(board.publication.availability, SurfaceAvailability::Fresh);
            assert!(board.validate().is_ok());
            assert!(shared_summary(&board).validate().is_ok());
        }
    }

    #[test]
    fn shared_publication_refuses_hostile_probe_reason() {
        let detection = detect_of("Surface Pro 6");
        let mut private = run_verify(&FakeProbes::default(), &detection);
        private.rows[0].reason = "bad\0reason".into();
        assert!(shared_board("surface-seat", &detection, &private, 42).is_err());
    }

    #[test]
    fn a_failing_subsystem_is_red_not_dropped() {
        let fake = FakeProbes {
            sam: Ok(SamReading {
                battery_readable: false,
                thermal_readable: false,
            }),
            ..FakeProbes::default()
        };
        let board = run_verify(&fake, &detect_of("Surface Pro 8"));
        assert_eq!(state_of(&board, Subsystem::Sam), ProbeState::Failed);
    }

    #[test]
    fn non_surface_verify_skips_cleanly_no_rows() {
        let dmi = DmiInfo {
            sys_vendor: "Dell Inc.".into(),
            product_name: "XPS 13".into(),
            product_sku: String::new(),
            ..Default::default()
        };
        let det = SurfaceDetection {
            model: identify(&dmi),
            dmi,
        };
        let board = run_verify(&FakeProbes::default(), &det);
        assert_eq!(board.skipped.as_deref(), Some("not a Microsoft Surface"));
        assert!(board.rows.is_empty());
    }

    // ── the summary computation ──────────────────────────────────────────────

    #[test]
    fn summary_all_green_is_100_percent_no_reds() {
        let board = run_verify(&FakeProbes::default(), &detect_of("Surface Pro 8"));
        let summary = summarize("node-x", &board);
        assert_eq!(summary.model, "Surface Pro 8");
        assert_eq!(summary.enablement_pct, 100);
        assert_eq!(summary.red_count, 0);
        assert!(summary.red_subsystems.is_empty());
    }

    #[test]
    fn summary_counts_reds_and_lowers_enablement() {
        // Two hard failures on an 8-subsystem Pro board → 6/8 green = 75%.
        let fake = FakeProbes {
            sam: Ok(SamReading {
                battery_readable: false,
                thermal_readable: false,
            }),
            wifi_bt: Ok(WifiBtReading {
                wifi_up: false,
                bt_up: false,
            }),
            ..FakeProbes::default()
        };
        let board = run_verify(&fake, &detect_of("Surface Pro 8"));
        let summary = summarize("node-x", &board);
        assert_eq!(summary.red_count, 2);
        assert!(summary.red_subsystems.contains(&"sam".to_string()));
        assert!(summary.red_subsystems.contains(&"wifi_bt".to_string()));
        assert_eq!(summary.enablement_pct, 75, "6 of 8 green");
    }

    #[test]
    fn summary_gesture_and_degraded_are_not_red_but_lower_enablement() {
        // Pen awaits a gesture, SAM degraded — neither is red, but neither is
        // green, so enablement drops without inflating red_count.
        let fake = FakeProbes {
            pen: Ok(PenReading {
                digitizer_present: true,
                pressure_seen: false,
                tilt_seen: false,
            }),
            sam: Ok(SamReading {
                battery_readable: true,
                thermal_readable: false,
            }),
            ..FakeProbes::default()
        };
        let board = run_verify(&fake, &detect_of("Surface Pro 8"));
        let summary = summarize("node-x", &board);
        assert_eq!(summary.red_count, 0, "gesture + degraded are not red");
        assert_eq!(summary.enablement_pct, 75, "6 of 8 fully green");
    }

    #[test]
    fn summary_of_an_empty_board_is_zero_percent() {
        let board = VerifyBoard::skipped("", "not a Microsoft Surface");
        let summary = summarize("node-x", &board);
        assert_eq!(summary.enablement_pct, 0);
        assert_eq!(summary.red_count, 0);
    }

    #[test]
    fn live_probes_are_environment_honest_and_never_request_private_data() {
        // The production seam must answer honestly headless. Depending on the
        // farm host, enumeration can be unavailable, empty, or successful.
        let board = run_verify(&LiveSurfaceProbes, &detect_of("Surface Laptop 3"));
        let camera = board
            .rows
            .iter()
            .find(|r| r.subsystem == Subsystem::Cameras)
            .expect("camera row");
        assert!(matches!(camera.state, ProbeState::Ok | ProbeState::Failed));
        let fp = board
            .rows
            .iter()
            .find(|r| r.subsystem == Subsystem::Fingerprint)
            .expect("fingerprint row");
        assert!(matches!(fp.state, ProbeState::Ok | ProbeState::Failed));
    }
}
