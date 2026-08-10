//! SURFACE-3 — the `surface_enable` worker + guided MOK enrollment.
//!
//! The day-2 *activation* half of the Microsoft Surface enablement epic
//! (design: `docs/design/surface-tablet-enablement.md`, locks #4 + #6).
//! SURFACE-2's [`crate::surface`] detection folds the DMI identity into a
//! per-model [`SurfaceProfile`]; this unit turns that profile into the
//! observed enablement posture the bootc image *can't* fully prove ahead of time:
//!
//! * **Activate + configure** — preserve the typed desired plan, but fail the
//!   live mutation closed until it is connected to the shared
//!   Preview/Commit/Cancel/Audit authority with a fresh provider generation.
//! * **Guided MOK enrollment** (lock #6) — on a Secure-Boot host whose
//!   linux-surface modules are blocked, stage the machine-owner key
//!   (`mokutil --import`), hand back the **exact blue MOK-Manager firmware
//!   copy** the operator will see, and require a **typed arming token**
//!   before the reboot (never an auto-reboot). After the reboot a fresh
//!   enable call re-classifies the state as [`MokState::Enrolled`] and
//!   verifies the modules load.
//!
//! **Everything that touches the machine sits behind the injectable
//! [`SurfaceActions`] seam.** The production seam ([`LiveSurfaceActions`])
//! uses fixed binaries, fixed paths, and bounded subprocesses for read-only
//! classification while every activation write remains integration-gated.
//! Pending enrollment is proven in-process by
//! matching the fixed package certificate's complete SHA-1 fingerprint against
//! `mokutil --list-new`; SHA-1 is used only because that is the exact identifier
//! mokutil exposes for this read, not as a trust primitive. MOK import and reboot
//! remain explicitly gated until the narrow credential helper and host-state
//! handoff exist. The pure core — the per-model [`plan_enable`], the
//! [`MokState`] machine, the [`run_enable`] fold — is unit-tested end-to-end
//! against a fake seam.
//!
//! This unit exposes the enable/MOK **result** types + the [`run_enable`]
//! verb reachably (SURFACE-4 publishes the enablement state to the fleet;
//! SURFACE-6 renders the Install tab from it). §6-clean: it stays wholly in
//! mackesd and reaches nothing in the desktop shell.

use std::path::Path;

#[cfg(feature = "async-services")]
use std::process::Command;
#[cfg(feature = "async-services")]
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha1::{Digest as _, Sha1};

use mackes_mesh_types::surface_hardware::SurfaceProGeneration;

use super::{Subsystem, SurfaceDetection, SurfaceDevice, SurfaceModel, SurfaceProfile};

// ─────────────────────────────── constants ──────────────────────────────────

/// The packaged per-device iptsd systemd template. A udev rule owns instance
/// selection; callers can never provide a unit or hidraw path.
pub const IPTSD_UNIT: &str = "iptsd@.service";

/// The linux-surface kernel modules the verify step confirms load once the
/// MOK key is enrolled. A representative core set (touch/pen digitizer + the
/// Surface Aggregator + the HID transport); the verify board (SURFACE-4)
/// refines per subsystem.
pub const SURFACE_MODULES: &[&str] = &["surface_aggregator", "surface_hid", "hid_multitouch"];

/// Fixed certificate shipped by the `surface-secureboot` package.
pub const MOK_KEY_PATH: &str = "/usr/share/surface-secureboot/surface.cer";

#[cfg(feature = "async-services")]
const SURFACE_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
/// Must match the bounded per-stream capture in `workers::proc`. Reaching the
/// limit is treated as truncation, never as a complete MOK-list proof.
const MOK_OUTPUT_LIMIT_BYTES: usize = 64 * 1024;
const SHA1_FINGERPRINT_BYTES: usize = 20;

/// The exact token the operator must type to arm the post-import reboot
/// (lock #6 — never an auto-reboot). Deliberately unambiguous; the Install
/// tab shows it and the enable request echoes it back in `arm_token`.
pub const MOK_ARM_TOKEN: &str = "REBOOT-TO-ENROLL-MOK";

// ─────────────────────────────── the seam ───────────────────────────────────

/// A typed configuration knob the enable plan applies. Each maps to a
/// specific linux-surface tuning; the seam applies the `value` for the key
/// (§9 — a typed verb, never a raw shell string).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConfigKey {
    /// Surface Aggregator platform perf profile (`low-power`/`balanced`/
    /// `performance`) — the SAM thermal/battery envelope.
    SamPerfProfile,
    /// iptsd touch/pen calibration + sensitivity profile.
    IptsdCalibration,
    /// Accelerometer → auto-rotation hint for the seat.
    RotationHint,
}

impl ConfigKey {
    /// Stable identifier for state keys / logs.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::SamPerfProfile => "sam_perf_profile",
            Self::IptsdCalibration => "iptsd_calibration",
            Self::RotationHint => "rotation_hint",
        }
    }
}

/// The current firmware Secure-Boot posture, as the enable flow needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecureBootState {
    /// Secure Boot is disabled — unsigned linux-surface modules load freely,
    /// so MOK enrollment is skipped entirely.
    Disabled,
    /// Secure Boot is enabled — modules must be signed by an enrolled key.
    Enabled,
}

/// The injectable seam over every machine-touching action the enable flow
/// performs (systemd, sysfs/config writes, mokutil, reboot). Tests hand a
/// fake; production hands [`LiveSurfaceActions`].
///
/// Every method is fallible with a typed [`EnableError`] so the fold records
/// an honest per-step outcome (applied / integration-gated / failed) — never
/// a silent success.
pub trait SurfaceActions {
    /// Enable + start a systemd unit (idempotent). `Ok(true)` when it was
    /// already active, `Ok(false)` when this call started it.
    fn enable_unit(&self, unit: &str) -> Result<bool, EnableError>;

    /// Apply one typed config knob's `value`.
    fn apply_config(&self, key: ConfigKey, value: &str) -> Result<(), EnableError>;

    /// Read the firmware Secure-Boot posture.
    fn secure_boot_state(&self) -> Result<SecureBootState, EnableError>;

    /// Is the machine-owner key (the one at [`MOK_KEY_PATH`]) already
    /// enrolled in the firmware MOK list?
    fn mok_enrolled(&self) -> Result<bool, EnableError>;

    /// Is the fixed certificate already staged in the firmware's pending MOK
    /// list? Reboot is refused unless this is positively proven.
    fn mok_pending(&self, key_path: &Path) -> Result<bool, EnableError>;

    /// Stage the key at `key_path` for enrollment (`mokutil --import`),
    /// returning the fingerprint the operator confirms at the blue screen.
    fn mok_import(&self, key_path: &Path) -> Result<String, EnableError>;

    /// Do the linux-surface `modules` all load right now? The post-reboot
    /// verify step.
    fn modules_loaded(&self, modules: &[&str]) -> Result<bool, EnableError>;

    /// Reboot the host. Only ever called after the typed arm matched.
    fn reboot(&self) -> Result<(), EnableError>;
}

/// A typed failure from the [`SurfaceActions`] seam.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnableError {
    /// The live action isn't wired to real hardware yet — the honest answer
    /// on any non-Surface dev box / CI (§7: never a faked success). `action`
    /// names what was gated.
    IntegrationGated {
        /// The action that is integration-gated (e.g. `"enable iptsd.service"`).
        action: String,
    },
    /// The live action ran and failed for a concrete reason.
    Failed {
        /// The action that failed.
        action: String,
        /// The underlying reason.
        detail: String,
    },
}

impl std::fmt::Display for EnableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IntegrationGated { action } => {
                write!(f, "{action}: integration-gated (live Surface hardware)")
            }
            Self::Failed { action, detail } => write!(f, "{action}: {detail}"),
        }
    }
}

impl std::error::Error for EnableError {}

/// The production Surface seam. Activation and read-only posture checks use
/// fixed, bounded host operations. MOK staging and reboot stay gated until
/// their dedicated credential, privilege, and host-state lanes land.
#[derive(Debug, Clone, Copy, Default)]
pub struct LiveSurfaceActions;

impl LiveSurfaceActions {
    fn gated<T>(action: impl Into<String>) -> Result<T, EnableError> {
        Err(EnableError::IntegrationGated {
            action: action.into(),
        })
    }

    fn failed<T>(action: impl Into<String>, detail: impl Into<String>) -> Result<T, EnableError> {
        Err(EnableError::Failed {
            action: action.into(),
            detail: detail.into(),
        })
    }
}

#[cfg(feature = "async-services")]
fn run_fixed(
    action: &str,
    program: &str,
    args: &[&str],
) -> Result<std::process::Output, EnableError> {
    let mut command = Command::new(program);
    command.args(args).env("LC_ALL", "C");
    crate::workers::proc::output_with_timeout(command, SURFACE_COMMAND_TIMEOUT).map_err(|error| {
        EnableError::Failed {
            action: action.to_string(),
            detail: error.to_string(),
        }
    })
}

fn parse_secure_boot_state(text: &str) -> Option<SecureBootState> {
    let lower = text.to_ascii_lowercase();
    if lower.contains("secureboot enabled") || lower.contains("secure boot enabled") {
        Some(SecureBootState::Enabled)
    } else if lower.contains("secureboot disabled") || lower.contains("secure boot disabled") {
        Some(SecureBootState::Disabled)
    } else {
        None
    }
}

fn parse_sha1_fingerprint(value: &str) -> Option<[u8; SHA1_FINGERPRINT_BYTES]> {
    let mut fingerprint = [0_u8; SHA1_FINGERPRINT_BYTES];
    let mut octets = value.split(':');
    for byte in &mut fingerprint {
        let octet = octets.next()?;
        if octet.len() != 2 || !octet.bytes().all(|value| value.is_ascii_hexdigit()) {
            return None;
        }
        *byte = u8::from_str_radix(octet, 16).ok()?;
    }
    if octets.next().is_some() {
        return None;
    }
    Some(fingerprint)
}

fn certificate_sha1(der: &[u8]) -> Result<[u8; SHA1_FINGERPRINT_BYTES], &'static str> {
    if der.is_empty() || der.len() >= MOK_OUTPUT_LIMIT_BYTES {
        return Err("Surface certificate is empty or truncated");
    }
    let digest = Sha1::digest(der);
    let mut fingerprint = [0_u8; SHA1_FINGERPRINT_BYTES];
    fingerprint.copy_from_slice(&digest);
    Ok(fingerprint)
}

fn pending_list_contains_sha1(
    output: &[u8],
    expected: &[u8; SHA1_FINGERPRINT_BYTES],
) -> Result<bool, &'static str> {
    if output.len() >= MOK_OUTPUT_LIMIT_BYTES {
        return Err("pending MOK list output is truncated");
    }
    let text = std::str::from_utf8(output).map_err(|_| "pending MOK list is not UTF-8")?;
    let mut fingerprints = Vec::new();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("SHA1 Fingerprint: ") {
            let fingerprint = parse_sha1_fingerprint(value)
                .ok_or("pending MOK list contains a malformed fingerprint")?;
            if fingerprints.contains(&fingerprint) {
                return Err("pending MOK list contains a duplicate fingerprint");
            }
            fingerprints.push(fingerprint);
        } else if line.starts_with("SHA1 Fingerprint:") {
            // Reject near-miss markers instead of silently treating corrupt or
            // attacker-controlled output as an unrelated certificate.
            return Err("pending MOK list contains a malformed fingerprint marker");
        }
    }
    Ok(fingerprints.iter().any(|value| value == expected))
}

impl SurfaceActions for LiveSurfaceActions {
    fn enable_unit(&self, unit: &str) -> Result<bool, EnableError> {
        if unit != IPTSD_UNIT {
            return Self::failed("activate iptsd", "unit is not the fixed iptsd template");
        }
        Self::gated(
            "activate iptsd (Surface Preview / Commit / Cancel / Audit authority unavailable)",
        )
    }

    fn apply_config(&self, key: ConfigKey, _value: &str) -> Result<(), EnableError> {
        if key != ConfigKey::SamPerfProfile {
            return Self::failed(
                format!("apply {}", key.id()),
                "configuration is package- or DRM-runner-owned",
            );
        }
        Self::gated(
            "apply sam_perf_profile (Surface Preview / Commit / Cancel / Audit authority unavailable)",
        )
    }

    fn secure_boot_state(&self) -> Result<SecureBootState, EnableError> {
        #[cfg(feature = "async-services")]
        {
            let output = run_fixed(
                "read secure-boot state",
                "/usr/bin/mokutil",
                &["--sb-state"],
            )?;
            let combined = format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            if !output.status.success() {
                return Self::failed("read secure-boot state", combined.trim());
            }
            return parse_secure_boot_state(&combined).ok_or_else(|| EnableError::Failed {
                action: "read secure-boot state".to_string(),
                detail: "mokutil returned an unrecognised state".to_string(),
            });
        }
        #[cfg(not(feature = "async-services"))]
        Self::gated("read secure-boot state (async-services disabled)")
    }

    fn mok_enrolled(&self) -> Result<bool, EnableError> {
        if !Path::new(MOK_KEY_PATH).is_file() {
            return Self::failed("query enrolled MOK key", "Surface certificate is missing");
        }
        #[cfg(feature = "async-services")]
        {
            let output = run_fixed(
                "query enrolled MOK key",
                "/usr/bin/mokutil",
                &["--test-key", MOK_KEY_PATH],
            )?;
            return Ok(output.status.success());
        }
        #[cfg(not(feature = "async-services"))]
        Self::gated("query enrolled MOK key (async-services disabled)")
    }

    fn mok_pending(&self, key_path: &Path) -> Result<bool, EnableError> {
        if key_path != Path::new(MOK_KEY_PATH) {
            return Self::failed(
                "query pending MOK key",
                "certificate path is not allowlisted",
            );
        }
        let metadata =
            std::fs::symlink_metadata(MOK_KEY_PATH).map_err(|error| EnableError::Failed {
                action: "query pending MOK key".to_string(),
                detail: format!("Surface certificate is unavailable: {error}"),
            })?;
        if !metadata.file_type().is_file()
            || metadata.len() == 0
            || metadata.len() > MOK_OUTPUT_LIMIT_BYTES as u64
        {
            return Self::failed(
                "query pending MOK key",
                "Surface certificate is not a bounded regular file",
            );
        }
        #[cfg(feature = "async-services")]
        {
            // `mokutil --list-new` exposes full SHA-1 fingerprints for pending
            // X.509 entries. Derive that same identifier from the one fixed
            // package certificate. This is an equality binding, not a claim
            // that SHA-1 provides modern collision resistance.
            let certificate = std::fs::read(MOK_KEY_PATH).map_err(|error| EnableError::Failed {
                action: "read Surface MOK certificate".to_string(),
                detail: error.to_string(),
            })?;
            let fingerprint =
                certificate_sha1(&certificate).map_err(|detail| EnableError::Failed {
                    action: "fingerprint Surface MOK certificate".to_string(),
                    detail: detail.to_string(),
                })?;

            let pending = run_fixed("query pending MOK key", "/usr/bin/mokutil", &["--list-new"])?;
            if !pending.status.success() || !pending.stderr.is_empty() {
                return Self::failed(
                    "query pending MOK key",
                    if pending.stderr.is_empty() {
                        "mokutil failed without diagnostic output".to_string()
                    } else {
                        String::from_utf8_lossy(&pending.stderr).trim().to_string()
                    },
                );
            }
            return pending_list_contains_sha1(&pending.stdout, &fingerprint).map_err(|detail| {
                EnableError::Failed {
                    action: "query pending MOK key".to_string(),
                    detail: detail.to_string(),
                }
            });
        }
        #[cfg(not(feature = "async-services"))]
        Self::gated("query pending MOK key (async-services disabled)")
    }

    fn mok_import(&self, key_path: &Path) -> Result<String, EnableError> {
        if key_path != Path::new(MOK_KEY_PATH) {
            return Self::failed("stage MOK key", "certificate path is not allowlisted");
        }
        // mokutil's only non-interactive import modes consume either a caller-
        // supplied password-hash file (`--hash-file`) or the host root password
        // hash (`--root-pw`). This action contract carries neither a sealed,
        // one-time credential nor an allowlisted helper capable of supplying
        // one. Using `/etc/shadow`, inventing a static password, accepting a
        // caller path, or putting a secret in argv/environment would violate
        // the credential boundary. Refuse before executing mokutil or mutating
        // EFI variables until a dedicated credential broker is part of the
        // typed request.
        Self::gated(
            "stage MOK key (sealed one-time credential broker is not implemented; no EFI mutation)",
        )
    }

    fn modules_loaded(&self, modules: &[&str]) -> Result<bool, EnableError> {
        if modules.is_empty()
            || modules.iter().any(|module| {
                module.is_empty()
                    || !module
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            })
        {
            return Self::failed("verify linux-surface modules", "invalid module allowlist");
        }
        Ok(modules
            .iter()
            .all(|module| Path::new("/sys/module").join(module).is_dir()))
    }

    fn reboot(&self) -> Result<(), EnableError> {
        Self::gated("reboot delegated to the typed host-state workflow")
    }
}

// ─────────────────────────── the enable plan (pure) ─────────────────────────

/// One config step in the enable plan — the knob, the value to write, and
/// the subsystem it serves (for the board).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigStep {
    /// The typed knob.
    pub key: ConfigKey,
    /// The value to apply.
    pub value: String,
    /// The subsystem this step enables.
    pub subsystem: Subsystem,
}

/// The per-model activation plan: which units to bring up and which config
/// knobs to apply. A pure fold over the [`SurfaceProfile`] — no I/O.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnablePlan {
    /// systemd units to enable + start (iptsd where the model has a
    /// touch/pen digitizer).
    pub units: Vec<String>,
    /// Config knobs to apply, in board order.
    pub configs: Vec<ConfigStep>,
}

/// Fold a recognised model's profile into its enable plan. Only the line
/// items the model actually *has* (per SURFACE-2's profile) produce steps —
/// a clamshell Laptop gets no rotation hint, a Studio gets no touch unit if
/// it lacked a digitizer, etc.
#[must_use]
pub fn plan_enable(device: &SurfaceDevice) -> EnablePlan {
    let p: &SurfaceProfile = &device.profile;
    let mut units = Vec::new();
    let mut configs = Vec::new();

    // The package owns model presets and udev owns the exact hidraw instance.
    // Activation only retriggers that fixed rule; it never accepts a device or
    // unit name from the caller.
    if p.touch || p.pen {
        units.push(IPTSD_UNIT.to_string());
    }

    // Surface Aggregator perf/thermal envelope.
    if p.sam {
        configs.push(ConfigStep {
            key: ConfigKey::SamPerfProfile,
            value: "balanced".to_string(),
            subsystem: Subsystem::Sam,
        });
    }

    // Rotation is applied by the DRM runner from live IIO/tablet state; there
    // is no host configuration write in the Surface enable action.

    EnablePlan { units, configs }
}

// ─────────────────────────── the MOK state machine (pure) ───────────────────

/// The Secure-Boot / MOK posture the enable flow classifies before acting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MokState {
    /// Secure Boot is off — unsigned modules load; nothing to enroll.
    NotSecureBoot,
    /// Secure Boot is on and the key is **not** enrolled — the modules are
    /// blocked until we import the key and reboot.
    KeyMissing,
    /// Secure Boot is on and the key **is** enrolled — verify the modules
    /// actually load.
    Enrolled,
}

/// The next action the MOK state dictates. The one-way flow is
/// `NotSecureBoot → Skip`; `KeyMissing → ImportThenArmReboot`; `Enrolled →
/// VerifyModules`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MokStep {
    /// Secure Boot off — skip MOK entirely.
    Skip,
    /// Stage the key, then require a typed-armed reboot.
    ImportThenArmReboot,
    /// Key enrolled — confirm the modules load.
    VerifyModules,
}

/// Classify the MOK posture from the firmware Secure-Boot state and whether
/// the key is already enrolled. Pure.
#[must_use]
pub const fn classify_mok(sb: SecureBootState, enrolled: bool) -> MokState {
    match sb {
        SecureBootState::Disabled => MokState::NotSecureBoot,
        SecureBootState::Enabled if enrolled => MokState::Enrolled,
        SecureBootState::Enabled => MokState::KeyMissing,
    }
}

/// The step a MOK state dictates. Pure.
#[must_use]
pub const fn mok_step(state: MokState) -> MokStep {
    match state {
        MokState::NotSecureBoot => MokStep::Skip,
        MokState::KeyMissing => MokStep::ImportThenArmReboot,
        MokState::Enrolled => MokStep::VerifyModules,
    }
}

/// Is the reboot armed — did the operator type the exact [`MOK_ARM_TOKEN`]?
/// Pure equality; a missing or wrong token is unarmed. This is the interlock
/// that makes the reboot never automatic (lock #6).
#[must_use]
pub fn is_armed(provided: Option<&str>, expected: &str) -> bool {
    provided.is_some_and(|t| t == expected)
}

/// The exact copy the blue MOK-Manager firmware screen presents after the
/// reboot — the manual step no software can automate (lock #6, "honest about
/// the manual firmware step"). Pure; the Install tab shows it verbatim.
#[must_use]
pub fn mok_firmware_prompt() -> String {
    format!(
        "After the reboot the firmware shows a blue \"Shim UEFI key management\" \
screen (MOK Manager). It will NOT continue to the desktop on its own:\n\
  1. Select \"Enroll MOK\"  →  \"Continue\".\n\
  2. Choose \"Yes\" to enroll the key.\n\
  3. Enter the one-time password you set during import (the same password \
mokutil asked for when staging the key).\n\
  4. Select \"Reboot\".\n\
If you miss the screen (it times out to the OS), re-run enable — the key is \
still staged until enrolled. Arm the reboot below by typing: {MOK_ARM_TOKEN}"
    )
}

// ─────────────────────────── result types ───────────────────────────────────

/// The outcome of one plan step (unit or config) against the seam.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepOutcome {
    /// Applied this call.
    Applied,
    /// Was already in the desired state (unit already active).
    AlreadyActive,
    /// The live action is integration-gated (honest, §7).
    Gated {
        /// The gated action's reason string.
        reason: String,
    },
    /// The live action ran and failed.
    Failed {
        /// The failure reason.
        reason: String,
    },
}

impl StepOutcome {
    /// Map a seam `Result` (with an `already`-active hint for units) to an
    /// outcome.
    fn from_unit(res: Result<bool, EnableError>) -> Self {
        match res {
            Ok(true) => Self::AlreadyActive,
            Ok(false) => Self::Applied,
            Err(e) => Self::from_err(&e),
        }
    }

    fn from_apply(res: Result<(), EnableError>) -> Self {
        match res {
            Ok(()) => Self::Applied,
            Err(e) => Self::from_err(&e),
        }
    }

    fn from_err(e: &EnableError) -> Self {
        match e {
            EnableError::IntegrationGated { .. } => Self::Gated {
                reason: e.to_string(),
            },
            EnableError::Failed { .. } => Self::Failed {
                reason: e.to_string(),
            },
        }
    }
}

/// One unit's activation record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitResult {
    /// The systemd unit.
    pub unit: String,
    /// Its outcome.
    pub outcome: StepOutcome,
}

/// One config knob's application record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigResult {
    /// The knob.
    pub key: ConfigKey,
    /// The subsystem it serves.
    pub subsystem: Subsystem,
    /// Its outcome.
    pub outcome: StepOutcome,
}

/// The activation half of the result (iptsd + config).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivationResult {
    /// Per-unit outcomes.
    pub units: Vec<UnitResult>,
    /// Per-config outcomes.
    pub configs: Vec<ConfigResult>,
}

/// The MOK-enrollment half of the result — the state machine's verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MokEnrollment {
    /// Secure Boot off — no enrollment needed.
    NotRequired,
    /// Key enrolled; whether the linux-surface modules load.
    Enrolled {
        /// Do the [`SURFACE_MODULES`] all load?
        modules_loaded: bool,
    },
    /// Key staged, awaiting the typed-armed reboot. Carries the exact
    /// firmware copy + the token the operator types to arm + the key
    /// fingerprint they confirm at the blue screen.
    ImportedAwaitingArm {
        /// The blue-screen firmware copy ([`mok_firmware_prompt`]).
        firmware_prompt: String,
        /// The arm token the operator must type ([`MOK_ARM_TOKEN`]).
        arm_token: String,
        /// The staged key's fingerprint (confirmed at the blue screen).
        key_fingerprint: String,
    },
    /// The typed arm matched and the reboot was issued (or gated live).
    RebootArmed {
        /// The reboot action's outcome.
        outcome: StepOutcome,
    },
    /// The MOK posture couldn't be determined (a gated/failed seam read).
    Undetermined {
        /// Why (the seam error).
        reason: String,
    },
}

/// The full typed result the `surface_enable` verb returns — what SURFACE-4
/// publishes and SURFACE-6's Install tab renders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnableResult {
    /// The recognised model's product string (empty when skipped).
    pub model: String,
    /// When set, enable was skipped and this is the honest reason (not a
    /// Surface / unrecognised Surface).
    pub skipped: Option<String>,
    /// The activation outcomes.
    pub activation: ActivationResult,
    /// The MOK-enrollment verdict.
    pub mok: MokEnrollment,
}

impl EnableResult {
    /// A skip result carrying the honest reason (non-Surface / unrecognised).
    fn skipped(model: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            skipped: Some(reason.into()),
            activation: ActivationResult::default(),
            mok: MokEnrollment::NotRequired,
        }
    }
}

// ─────────────────────────── the verb (fold over the seam) ──────────────────

/// The `surface_enable` verb: activate + configure this node per its detected
/// model, then walk the guided MOK state machine. Pure control flow over the
/// injectable [`SurfaceActions`] seam — the whole thing is unit-tested with a
/// fake; production hands [`LiveSurfaceActions`] (integration-gated).
///
/// `arm` is the operator-typed arming token from the enable request; it only
/// matters in the [`MokState::KeyMissing`] branch, where a matching token
/// (see [`is_armed`]) triggers the reboot and anything else stages the key +
/// returns the firmware copy.
///
/// A non-Surface (or unrecognised-Surface) node is skipped cleanly — no
/// actions, an honest `skipped` reason, no MOK.
#[must_use]
pub fn run_enable(
    actions: &impl SurfaceActions,
    detection: &SurfaceDetection,
    arm: Option<&str>,
) -> EnableResult {
    let device = match &detection.model {
        SurfaceModel::NotASurface => {
            return EnableResult::skipped("", "not a Microsoft Surface");
        }
        SurfaceModel::UnknownSurface { product } => {
            return EnableResult::skipped(
                product.clone(),
                format!("unrecognised Surface: {product} (no per-model profile)"),
            );
        }
        SurfaceModel::Known(dev) => dev,
    };
    if !matches!(
        device.contract_generation,
        SurfaceProGeneration::Pro5 | SurfaceProGeneration::Pro6
    ) {
        return EnableResult::skipped(
            device.product.clone(),
            format!(
                "{} is detected but not admitted by the Surface Pro 5/6 action contract",
                device.product
            ),
        );
    }

    let activation = run_activation(actions, device);
    let mok = run_mok(actions, arm);

    EnableResult {
        model: device.product.clone(),
        skipped: None,
        activation,
        mok,
    }
}

/// Apply the per-model plan (units + config) against the seam.
fn run_activation(actions: &impl SurfaceActions, device: &SurfaceDevice) -> ActivationResult {
    let plan = plan_enable(device);
    let units = plan
        .units
        .into_iter()
        .map(|unit| {
            let outcome = StepOutcome::from_unit(actions.enable_unit(&unit));
            UnitResult { unit, outcome }
        })
        .collect();
    let configs = plan
        .configs
        .into_iter()
        .map(|step| {
            let outcome = StepOutcome::from_apply(actions.apply_config(step.key, &step.value));
            ConfigResult {
                key: step.key,
                subsystem: step.subsystem,
                outcome,
            }
        })
        .collect();
    ActivationResult { units, configs }
}

/// Walk the guided MOK state machine against the seam.
fn run_mok(actions: &impl SurfaceActions, arm: Option<&str>) -> MokEnrollment {
    // Classify: read Secure-Boot posture + enrollment.
    let sb = match actions.secure_boot_state() {
        Ok(sb) => sb,
        Err(e) => {
            return MokEnrollment::Undetermined {
                reason: e.to_string(),
            };
        }
    };
    // The enrollment query only matters when Secure Boot is on; skip it (and
    // its possible gated error) when SB is off.
    let enrolled = match sb {
        SecureBootState::Disabled => false,
        SecureBootState::Enabled => match actions.mok_enrolled() {
            Ok(e) => e,
            Err(e) => {
                return MokEnrollment::Undetermined {
                    reason: e.to_string(),
                };
            }
        },
    };

    match mok_step(classify_mok(sb, enrolled)) {
        MokStep::Skip => MokEnrollment::NotRequired,
        MokStep::VerifyModules => match actions.modules_loaded(SURFACE_MODULES) {
            Ok(modules_loaded) => MokEnrollment::Enrolled { modules_loaded },
            Err(e) => MokEnrollment::Undetermined {
                reason: e.to_string(),
            },
        },
        MokStep::ImportThenArmReboot => {
            if is_armed(arm, MOK_ARM_TOKEN) {
                // A typed token alone is insufficient: first prove that the
                // fixed certificate is actually in the pending enrollment
                // list. This prevents a reboot after a failed/skipped import.
                match actions.mok_pending(Path::new(MOK_KEY_PATH)) {
                    Ok(true) => {}
                    Ok(false) => {
                        return MokEnrollment::Undetermined {
                            reason:
                                "MOK reboot refused: Surface certificate is not pending enrollment"
                                    .to_string(),
                        };
                    }
                    Err(error) => {
                        return MokEnrollment::Undetermined {
                            reason: error.to_string(),
                        };
                    }
                }
                MokEnrollment::RebootArmed {
                    outcome: StepOutcome::from_apply(actions.reboot()),
                }
            } else {
                // Stage the key and hand back the firmware copy; do NOT
                // reboot (lock #6). A gated/failed import is honest too.
                match actions.mok_import(Path::new(MOK_KEY_PATH)) {
                    Ok(key_fingerprint) => MokEnrollment::ImportedAwaitingArm {
                        firmware_prompt: mok_firmware_prompt(),
                        arm_token: MOK_ARM_TOKEN.to_string(),
                        key_fingerprint,
                    },
                    Err(e) => MokEnrollment::Undetermined {
                        reason: e.to_string(),
                    },
                }
            }
        }
    }
}

// ─────────────────────────── the Bus worker (per-node) ──────────────────────

#[cfg(feature = "async-services")]
pub use worker::{
    ENABLE_ACTION_AUTH_VERB, EnableRequest, SurfaceEnableWorker, enable_topic, result_topic,
};

#[cfg(feature = "async-services")]
mod worker {
    //! The per-node `surface_enable` Bus worker (a *leader-of-self* worker:
    //! it acts only on its own hardware, never a remote node). It drains
    //! [`enable_topic`] for this node, runs [`super::run_enable`] against the
    //! integration-gated [`super::LiveSurfaceActions`], and publishes the
    //! typed [`super::EnableResult`] to [`result_topic`]. SURFACE-4 folds
    //! that into the fleet enablement summary.

    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    pub use mackes_mesh_types::surface_hardware::SurfaceEnableRequest as EnableRequest;
    use mde_bus::hooks::config::Priority;
    use mde_bus::persist::Persist;

    use super::{EnableResult, LiveSurfaceActions, SurfaceModel, run_enable};
    use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};
    use crate::surface::{SurfaceDetection, detect};
    use crate::workers::{ShutdownToken, Worker};

    /// Poll cadence — enable is operator-driven, so a modest tick is plenty.
    pub const POLL: Duration = Duration::from_secs(2);

    /// Closed semantic verb bound into every surface enable capability.
    /// Publishers must mint schema-v1 HMAC authority for this verb, the target
    /// node, and that same node as the mutation target.
    pub const ENABLE_ACTION_AUTH_VERB: &str = "surface-enable";

    /// The per-node request lane the Install tab publishes enable requests on.
    #[must_use]
    pub fn enable_topic(node: &str) -> String {
        format!("action/hardware/surface/{node}/enable")
    }

    /// The per-node result lane the typed [`EnableResult`] lands on.
    #[must_use]
    pub fn result_topic(node: &str) -> String {
        format!("state/hardware/surface/{node}/enable")
    }

    /// The per-node `surface_enable` worker.
    pub struct SurfaceEnableWorker {
        node_id: String,
        detection: SurfaceDetection,
        bus_root: Option<PathBuf>,
        poll: Duration,
        action_cursor: Option<String>,
        authorizer: Arc<ActionAuthorizer>,
    }

    impl SurfaceEnableWorker {
        /// Build the worker for `node_id`, detecting this host's Surface
        /// identity now (SURFACE-2's [`detect`]).
        #[must_use]
        pub fn new(node_id: String) -> Self {
            Self {
                node_id,
                detection: detect(),
                bus_root: default_bus_root(),
                poll: POLL,
                action_cursor: None,
                authorizer: Arc::new(ActionAuthorizer::production()),
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
                action_cursor: None,
                authorizer: Arc::new(ActionAuthorizer::production()),
            }
        }

        /// Test constructor with an injectable shared action authorizer.
        #[cfg(test)]
        #[must_use]
        pub(crate) fn with_parts_and_authorizer(
            node_id: String,
            detection: SurfaceDetection,
            bus_root: PathBuf,
            authorizer: Arc<ActionAuthorizer>,
        ) -> Self {
            Self {
                node_id,
                detection,
                bus_root: Some(bus_root),
                poll: POLL,
                action_cursor: None,
                authorizer,
            }
        }

        /// Drain any new enable requests, run the verb, publish the result.
        /// Pulled out so a test drives it against a temp Bus without the run
        /// loop / clock.
        fn poll_once(&mut self, persist: &Persist) {
            let topic = enable_topic(&self.node_id);
            let Ok(msgs) = persist.list_since(&topic, self.action_cursor.as_deref()) else {
                return;
            };
            for msg in msgs {
                self.action_cursor = Some(msg.ulid.clone());
                let result = self.apply_request(msg.body.as_deref());
                self.publish(persist, &result);
            }
        }

        /// Authenticate and decode one raw Bus request, then run the typed
        /// enable verb. Parsing is side-effect free; the shared exact-body
        /// gate runs before [`run_enable`] or any privileged seam call.
        fn apply_request(&self, body: Option<&str>) -> EnableResult {
            let Some(body) = body else {
                return self.refused_result("enable request body is missing");
            };
            let req =
                match EnableRequest::from_json_at(body.as_bytes(), &self.node_id, wall_now_ms()) {
                    Ok(req) => req,
                    Err(error) => {
                        return self.refused_result(&format!(
                            "enable request failed shared contract admission: {error}"
                        ));
                    }
                };
            let context = MutationContext {
                verb: ENABLE_ACTION_AUTH_VERB,
                node: &self.node_id,
                target: &self.node_id,
            };
            if let Err(error) = self.authorizer.authorize(body, context) {
                tracing::warn!(
                    target: "mackesd::surface_enable",
                    node = %self.node_id,
                    %error,
                    "refused unauthorized surface enable"
                );
                return self
                    .refused_result(&format!("surface enable authorization refused: {error}"));
            }
            run_enable(
                &LiveSurfaceActions,
                &self.detection,
                req.arm_token.as_deref(),
            )
        }

        fn refused_result(&self, reason: &str) -> EnableResult {
            let model = match &self.detection.model {
                SurfaceModel::Known(device) => device.product.clone(),
                SurfaceModel::UnknownSurface { product } => product.clone(),
                SurfaceModel::NotASurface => String::new(),
            };
            EnableResult::skipped(model, reason)
        }

        /// Publish the typed result to the per-node result lane.
        fn publish(&self, persist: &Persist, result: &EnableResult) {
            let Ok(body) = serde_json::to_string(result) else {
                return;
            };
            let topic = result_topic(&self.node_id);
            if let Err(e) = persist.write(&topic, Priority::Default, None, Some(&body)) {
                tracing::debug!(
                    target: "mackesd::surface_enable",
                    error = %e,
                    "enable result publish failed"
                );
            }
        }
    }

    fn wall_now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }

    /// The default Bus root (same shape the other bus workers use).
    fn default_bus_root() -> Option<PathBuf> {
        mde_bus::default_data_dir()
    }

    #[async_trait::async_trait]
    impl Worker for SurfaceEnableWorker {
        fn name(&self) -> &'static str {
            "surface_enable"
        }

        async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
            // Non-Surface node: the card never appears, so the worker idles
            // (it never touches the Bus) rather than spin.
            if !self.detection.model.is_surface() {
                tracing::debug!(
                    target: "mackesd::surface_enable",
                    "not a Surface; worker idle"
                );
                shutdown.wait().await;
                return Ok(());
            }
            let Some(root) = self.bus_root.clone() else {
                tracing::debug!(target: "mackesd::surface_enable", "no bus root; worker idle");
                shutdown.wait().await;
                return Ok(());
            };
            loop {
                match Persist::open(root.clone()) {
                    Ok(persist) => self.poll_once(&persist),
                    Err(e) => tracing::debug!(
                        target: "mackesd::surface_enable",
                        error = %e,
                        "bus open failed"
                    ),
                }
                tokio::select! {
                    () = tokio::time::sleep(self.poll) => {}
                    () = shutdown.wait() => return Ok(()),
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use std::sync::Arc;

        use super::*;
        use crate::ipc::action_auth::{ActionAuthorizer, MutationContext, authorize_test_body};
        use crate::surface::{DmiInfo, MS_VENDOR, identify};

        const AUTH_KEY: &[u8] = b"surface-enable-action-auth-test-key";
        const AUTH_NOW: i64 = 1_700_000_000_000;

        fn detection(product: &str) -> SurfaceDetection {
            let dmi = DmiInfo {
                sys_vendor: MS_VENDOR.to_string(),
                product_name: product.to_string(),
                product_sku: String::new(),
                ..Default::default()
            };
            SurfaceDetection {
                model: identify(&dmi),
                dmi,
            }
        }

        #[test]
        fn default_bus_root_honors_mde_bus_root() {
            let root = tempfile::tempdir().expect("tempdir");
            let expected = root.path().to_path_buf();
            let previous = std::env::var_os("MDE_BUS_ROOT");
            let got = {
                std::env::set_var("MDE_BUS_ROOT", &expected);
                let got = SurfaceEnableWorker::new("node-a".into()).bus_root;
                match previous {
                    Some(value) => std::env::set_var("MDE_BUS_ROOT", value),
                    None => std::env::remove_var("MDE_BUS_ROOT"),
                }
                got
            };

            assert_eq!(got, Some(expected));
        }

        fn authorized_worker(
            node: &str,
            detection: SurfaceDetection,
            root: &std::path::Path,
        ) -> SurfaceEnableWorker {
            let authorizer = Arc::new(ActionAuthorizer::for_test(
                AUTH_KEY,
                root.join("auth"),
                AUTH_NOW,
            ));
            SurfaceEnableWorker::with_parts_and_authorizer(
                node.to_string(),
                detection,
                root.to_path_buf(),
                authorizer,
            )
        }

        fn signed_request(node: &str, arm_token: Option<&str>, nonce: &str) -> String {
            let unsigned = serde_json::to_string(&EnableRequest {
                header: mackes_mesh_types::surface_hardware::SurfaceActionHeader {
                    schema_version:
                        mackes_mesh_types::surface_hardware::SURFACE_HARDWARE_SCHEMA_VERSION,
                    node: node.to_string(),
                    request_id: nonce.to_string(),
                    issued_at_ms: wall_now_ms(),
                    armed_token: None,
                },
                arm_token: arm_token.map(str::to_string),
            })
            .expect("serialize shared enable request");
            authorize_test_body(
                AUTH_KEY,
                &unsigned,
                MutationContext {
                    verb: ENABLE_ACTION_AUTH_VERB,
                    node,
                    target: node,
                },
                nonce,
                AUTH_NOW + 30_000,
            )
        }

        #[test]
        fn drains_a_request_and_publishes_a_result() {
            let dir = tempfile::tempdir().expect("tempdir");
            let persist = Persist::open(dir.path().to_path_buf()).expect("open bus");
            // Use a detected-but-unsupported model so this worker plumbing
            // test can never touch host activation paths on a Surface farm
            // node. Live Pro 5/6 effects require a dedicated hardware gate.
            let mut w = authorized_worker("node-a", detection("Surface Pro 8"), dir.path());

            // The Install tab requests enable (no arm token).
            let req = signed_request("node-a", None, "surface-enable-valid");
            persist
                .write(&enable_topic("node-a"), Priority::Default, None, Some(&req))
                .expect("write request");

            w.poll_once(&persist);

            let out = persist
                .list_since(&result_topic("node-a"), None)
                .expect("list results");
            assert_eq!(out.len(), 1, "one result published");
            let result: EnableResult =
                serde_json::from_str(out[0].body.as_deref().unwrap()).unwrap();
            assert_eq!(result.model, "Surface Pro 8");
            assert!(result.skipped.as_deref().is_some_and(|reason| {
                reason.contains("not admitted by the Surface Pro 5/6 action contract")
            }));
        }

        #[test]
        fn cursor_advances_so_a_request_is_processed_once() {
            let dir = tempfile::tempdir().expect("tempdir");
            let persist = Persist::open(dir.path().to_path_buf()).expect("open bus");
            let mut w = authorized_worker("n", detection("Surface Pro 6"), dir.path());
            let req = signed_request("n", None, "surface-enable-cursor");
            persist
                .write(&enable_topic("n"), Priority::Default, None, Some(&req))
                .expect("write");
            w.poll_once(&persist);
            w.poll_once(&persist); // second drain: nothing new
            let out = persist.list_since(&result_topic("n"), None).expect("list");
            assert_eq!(out.len(), 1, "request processed exactly once");
        }

        #[test]
        fn typed_enable_without_hmac_capability_is_refused_before_actions() {
            let dir = tempfile::tempdir().expect("tempdir");
            let persist = Persist::open(dir.path().to_path_buf()).expect("open bus");
            let mut w = SurfaceEnableWorker::with_parts(
                "node-a".into(),
                detection("Surface Pro 6"),
                dir.path().to_path_buf(),
            );
            let request = serde_json::json!({
                "schema_version": 1,
                "arm_token": super::super::MOK_ARM_TOKEN,
            })
            .to_string();
            persist
                .write(
                    &enable_topic("node-a"),
                    Priority::Default,
                    None,
                    Some(&request),
                )
                .expect("write request");

            w.poll_once(&persist);

            let out = persist
                .list_since(&result_topic("node-a"), None)
                .expect("list results");
            let result: EnableResult =
                serde_json::from_str(out[0].body.as_deref().unwrap()).unwrap();
            assert!(
                result
                    .skipped
                    .as_deref()
                    .is_some_and(|reason| reason.contains("shared contract admission"))
            );
            assert!(result.activation.units.is_empty());
        }

        #[test]
        fn foreign_and_stale_shared_enable_requests_are_refused_before_authorization() {
            let dir = tempfile::tempdir().expect("tempdir");
            let worker = authorized_worker("node-a", detection("Surface Pro 6"), dir.path());
            let request = EnableRequest {
                header: mackes_mesh_types::surface_hardware::SurfaceActionHeader {
                    schema_version:
                        mackes_mesh_types::surface_hardware::SURFACE_HARDWARE_SCHEMA_VERSION,
                    node: "node-b".into(),
                    request_id: "foreign-enable".into(),
                    issued_at_ms: 1,
                    armed_token: None,
                },
                arm_token: None,
            };
            let result = worker.apply_request(Some(
                &serde_json::to_string(&request).expect("serialize hostile request"),
            ));
            assert!(
                result
                    .skipped
                    .as_deref()
                    .is_some_and(|reason| reason.contains("targets a different node"))
            );
            assert!(result.activation.units.is_empty());

            let stale = EnableRequest {
                header: mackes_mesh_types::surface_hardware::SurfaceActionHeader {
                    schema_version:
                        mackes_mesh_types::surface_hardware::SURFACE_HARDWARE_SCHEMA_VERSION,
                    node: "node-a".into(),
                    request_id: "stale-enable".into(),
                    issued_at_ms: 1,
                    armed_token: None,
                },
                arm_token: None,
            };
            let result = worker.apply_request(Some(
                &serde_json::to_string(&stale).expect("serialize stale request"),
            ));
            assert!(
                result
                    .skipped
                    .as_deref()
                    .is_some_and(|reason| reason.contains("stale or future-dated"))
            );
            assert!(result.activation.units.is_empty());
        }

        #[test]
        fn body_binding_and_single_use_are_enforced_for_enable() {
            let dir = tempfile::tempdir().expect("tempdir");
            let persist = Persist::open(dir.path().to_path_buf()).expect("open bus");
            let mut w = authorized_worker("node-replay", detection("Surface Pro 6"), dir.path());
            let original = signed_request("node-replay", None, "surface-enable-replay");
            let tampered = original.replace(
                "\"arm_token\":null",
                "\"arm_token\":\"REBOOT-TO-ENROLL-MOK\"",
            );
            for request in [&tampered, &original, &original] {
                persist
                    .write(
                        &enable_topic("node-replay"),
                        Priority::Default,
                        None,
                        Some(request),
                    )
                    .expect("write request");
            }

            w.poll_once(&persist);

            let out = persist
                .list_since(&result_topic("node-replay"), None)
                .expect("list results");
            assert_eq!(out.len(), 3);
            let results: Vec<EnableResult> = out
                .iter()
                .map(|item| serde_json::from_str(item.body.as_deref().unwrap()).unwrap())
                .collect();
            assert!(
                results[0]
                    .skipped
                    .as_deref()
                    .is_some_and(|reason| reason.contains("authorization refused"))
            );
            assert!(results[1].skipped.is_none());
            assert!(
                results[2]
                    .skipped
                    .as_deref()
                    .is_some_and(|reason| reason.contains("already used"))
            );
        }
    }
}

// ─────────────────────────────── tests ──────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::{DmiInfo, MS_VENDOR, SurfaceFamily, identify};

    /// A fake seam whose every action is scripted, so the fold + state
    /// machine run green without touching a machine.
    #[derive(Default)]
    struct FakeActions {
        secure_boot: Option<SecureBootState>,
        enrolled: bool,
        pending: bool,
        modules_loaded: bool,
        import_fingerprint: Option<String>,
        // failure injection
        enable_fails: bool,
        sb_read_fails: bool,
    }

    impl SurfaceActions for FakeActions {
        fn enable_unit(&self, unit: &str) -> Result<bool, EnableError> {
            if self.enable_fails {
                return Err(EnableError::Failed {
                    action: format!("enable {unit}"),
                    detail: "unit masked".into(),
                });
            }
            Ok(false)
        }
        fn apply_config(&self, _key: ConfigKey, _value: &str) -> Result<(), EnableError> {
            Ok(())
        }
        fn secure_boot_state(&self) -> Result<SecureBootState, EnableError> {
            if self.sb_read_fails {
                return Err(EnableError::IntegrationGated {
                    action: "read secure-boot state".into(),
                });
            }
            Ok(self.secure_boot.unwrap_or(SecureBootState::Disabled))
        }
        fn mok_enrolled(&self) -> Result<bool, EnableError> {
            Ok(self.enrolled)
        }
        fn mok_pending(&self, _key_path: &Path) -> Result<bool, EnableError> {
            Ok(self.pending)
        }
        fn mok_import(&self, _key_path: &Path) -> Result<String, EnableError> {
            Ok(self
                .import_fingerprint
                .clone()
                .unwrap_or_else(|| "AA:BB:CC".into()))
        }
        fn modules_loaded(&self, _modules: &[&str]) -> Result<bool, EnableError> {
            Ok(self.modules_loaded)
        }
        fn reboot(&self) -> Result<(), EnableError> {
            Ok(())
        }
    }

    fn detect_of(product: &str) -> SurfaceDetection {
        let dmi = DmiInfo {
            sys_vendor: MS_VENDOR.to_string(),
            product_name: product.to_string(),
            product_sku: String::new(),
            ..Default::default()
        };
        SurfaceDetection {
            model: identify(&dmi),
            dmi,
        }
    }

    fn device_of(product: &str) -> SurfaceDevice {
        let (product_name, product_sku) = if product == "Surface Pro 5" {
            ("Surface Pro", "Surface_Pro_1796")
        } else {
            (product, "")
        };
        match identify(&DmiInfo {
            sys_vendor: MS_VENDOR.to_string(),
            product_name: product_name.to_string(),
            product_sku: product_sku.to_string(),
            ..Default::default()
        }) {
            SurfaceModel::Known(dev) => dev,
            other => panic!("expected Known, got {other:?}"),
        }
    }

    // ── the per-model enable plan ───────────────────────────────────────────

    #[test]
    fn plan_for_pro5_and_pro6_use_udev_iptsd_and_sam_only() {
        for product in ["Surface Pro 5", "Surface Pro 6"] {
            let plan = plan_enable(&device_of(product));
            assert_eq!(plan.units, vec![IPTSD_UNIT.to_string()]);
            assert_eq!(plan.configs.len(), 1);
            assert_eq!(plan.configs[0].key, ConfigKey::SamPerfProfile);
            assert_eq!(plan.configs[0].value, "balanced");
        }
    }

    #[test]
    fn iptsd_presets_and_rotation_are_not_host_config_writes() {
        let plan = plan_enable(&device_of("Surface Pro 6"));
        assert_eq!(plan.units, vec![IPTSD_UNIT.to_string()]);
        let keys: Vec<_> = plan.configs.iter().map(|c| c.key).collect();
        assert!(keys.contains(&ConfigKey::SamPerfProfile));
        assert!(!keys.contains(&ConfigKey::IptsdCalibration));
        assert!(!keys.contains(&ConfigKey::RotationHint));
    }

    #[test]
    fn plan_for_laptop_has_no_rotation_hint() {
        let plan = plan_enable(&device_of("Surface Laptop 3"));
        // Still gets udev-managed iptsd and SAM, but no host rotation write.
        assert_eq!(plan.units, vec![IPTSD_UNIT.to_string()]);
        let keys: Vec<_> = plan.configs.iter().map(|c| c.key).collect();
        assert!(keys.contains(&ConfigKey::SamPerfProfile));
        assert!(
            !keys.contains(&ConfigKey::RotationHint),
            "the clamshell Laptop doesn't auto-rotate"
        );
    }

    #[test]
    fn plan_for_studio_has_no_rotation_hint_either() {
        let plan = plan_enable(&device_of("Surface Studio 2"));
        let keys: Vec<_> = plan.configs.iter().map(|c| c.key).collect();
        assert!(!keys.contains(&ConfigKey::RotationHint));
        assert!(keys.contains(&ConfigKey::SamPerfProfile));
        assert_eq!(device_of("Surface Studio 2").family, SurfaceFamily::Studio);
    }

    // ── the MOK state machine (all branches) ────────────────────────────────

    #[test]
    fn classify_sb_off_is_not_secure_boot() {
        assert_eq!(
            classify_mok(SecureBootState::Disabled, false),
            MokState::NotSecureBoot
        );
        assert_eq!(mok_step(MokState::NotSecureBoot), MokStep::Skip);
    }

    #[test]
    fn classify_sb_on_unenrolled_is_key_missing() {
        assert_eq!(
            classify_mok(SecureBootState::Enabled, false),
            MokState::KeyMissing
        );
        assert_eq!(mok_step(MokState::KeyMissing), MokStep::ImportThenArmReboot);
    }

    #[test]
    fn classify_sb_on_enrolled_is_enrolled() {
        assert_eq!(
            classify_mok(SecureBootState::Enabled, true),
            MokState::Enrolled
        );
        assert_eq!(mok_step(MokState::Enrolled), MokStep::VerifyModules);
    }

    #[test]
    fn arm_requires_the_exact_token() {
        assert!(is_armed(Some(MOK_ARM_TOKEN), MOK_ARM_TOKEN));
        assert!(!is_armed(Some("reboot"), MOK_ARM_TOKEN));
        assert!(!is_armed(None, MOK_ARM_TOKEN));
    }

    #[test]
    fn firmware_prompt_names_enroll_mok_and_the_arm_token() {
        let copy = mok_firmware_prompt();
        assert!(copy.contains("Enroll MOK"));
        assert!(copy.contains("one-time password"));
        assert!(copy.contains(MOK_ARM_TOKEN));
    }

    // ── the run_enable fold (each MOK branch, with a fake seam) ──────────────

    #[test]
    fn non_surface_skips_cleanly_no_actions_no_mok() {
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
        let r = run_enable(&FakeActions::default(), &det, None);
        assert_eq!(r.skipped.as_deref(), Some("not a Microsoft Surface"));
        assert!(r.activation.units.is_empty());
        assert_eq!(r.mok, MokEnrollment::NotRequired);
    }

    #[test]
    fn unrecognised_surface_skips_with_honest_reason() {
        let dmi = DmiInfo {
            sys_vendor: MS_VENDOR.to_string(),
            product_name: "Surface Duo".into(),
            product_sku: String::new(),
            ..Default::default()
        };
        let det = SurfaceDetection {
            model: identify(&dmi),
            dmi,
        };
        let r = run_enable(&FakeActions::default(), &det, None);
        assert!(
            r.skipped
                .as_deref()
                .unwrap()
                .contains("unrecognised Surface")
        );
    }

    #[test]
    fn detected_but_unsupported_surface_generation_never_reaches_actions() {
        let result = run_enable(
            &FakeActions {
                secure_boot: Some(SecureBootState::Enabled),
                pending: true,
                ..Default::default()
            },
            &detect_of("Surface Pro 7"),
            Some(MOK_ARM_TOKEN),
        );
        assert!(
            result
                .skipped
                .as_deref()
                .is_some_and(|reason| reason.contains("Pro 5/6 action contract"))
        );
        assert!(result.activation.units.is_empty());
        assert!(result.activation.configs.is_empty());
        assert_eq!(result.mok, MokEnrollment::NotRequired);
    }

    #[test]
    fn sb_off_activates_and_skips_mok() {
        let fake = FakeActions {
            secure_boot: Some(SecureBootState::Disabled),
            ..Default::default()
        };
        let r = run_enable(&fake, &detect_of("Surface Pro 6"), None);
        assert_eq!(r.model, "Surface Pro 6");
        assert_eq!(r.activation.units[0].outcome, StepOutcome::Applied);
        assert!(
            r.activation
                .configs
                .iter()
                .all(|c| c.outcome == StepOutcome::Applied)
        );
        assert_eq!(r.mok, MokEnrollment::NotRequired);
    }

    #[test]
    fn sb_on_unenrolled_no_arm_stages_key_and_returns_firmware_copy() {
        let fake = FakeActions {
            secure_boot: Some(SecureBootState::Enabled),
            enrolled: false,
            import_fingerprint: Some("12:34:56".into()),
            ..Default::default()
        };
        let r = run_enable(&fake, &detect_of("Surface Pro 6"), None);
        match r.mok {
            MokEnrollment::ImportedAwaitingArm {
                firmware_prompt,
                arm_token,
                key_fingerprint,
            } => {
                assert_eq!(arm_token, MOK_ARM_TOKEN);
                assert_eq!(key_fingerprint, "12:34:56");
                assert!(firmware_prompt.contains("Enroll MOK"));
            }
            other => panic!("expected ImportedAwaitingArm, got {other:?}"),
        }
    }

    #[test]
    fn sb_on_unenrolled_with_arm_issues_the_reboot() {
        let fake = FakeActions {
            secure_boot: Some(SecureBootState::Enabled),
            enrolled: false,
            pending: true,
            ..Default::default()
        };
        let r = run_enable(&fake, &detect_of("Surface Pro 6"), Some(MOK_ARM_TOKEN));
        assert_eq!(
            r.mok,
            MokEnrollment::RebootArmed {
                outcome: StepOutcome::Applied
            }
        );
    }

    #[test]
    fn sb_on_unenrolled_with_arm_refuses_reboot_without_pending_proof() {
        let fake = FakeActions {
            secure_boot: Some(SecureBootState::Enabled),
            enrolled: false,
            pending: false,
            ..Default::default()
        };
        let result = run_enable(&fake, &detect_of("Surface Pro 6"), Some(MOK_ARM_TOKEN));
        let MokEnrollment::Undetermined { reason } = result.mok else {
            panic!("reboot was not refused without pending proof");
        };
        assert!(reason.contains("not pending enrollment"));
    }

    #[test]
    fn secure_boot_parser_fails_closed() {
        assert_eq!(
            parse_secure_boot_state("SecureBoot enabled"),
            Some(SecureBootState::Enabled)
        );
        assert_eq!(
            parse_secure_boot_state("SecureBoot disabled"),
            Some(SecureBootState::Disabled)
        );
        assert_eq!(parse_secure_boot_state("unknown"), None);
    }

    #[test]
    fn pending_mok_parser_binds_the_complete_certificate_fingerprint() {
        let expected =
            parse_sha1_fingerprint("01:23:45:67:89:AB:CD:EF:10:32:54:76:98:BA:DC:FE:11:22:33:44")
                .expect("valid fixture fingerprint");
        let output = b"[key 1]\nOwner: 00000000-0000-0000-0000-000000000000\n\
SHA1 Fingerprint: AA:AA:AA:AA:AA:AA:AA:AA:AA:AA:AA:AA:AA:AA:AA:AA:AA:AA:AA:AA\n\
Certificate:\n    Data:\n\n[key 2]\nOwner: 11111111-1111-1111-1111-111111111111\n\
SHA1 Fingerprint: 01:23:45:67:89:AB:CD:EF:10:32:54:76:98:BA:DC:FE:11:22:33:44\n";
        assert_eq!(pending_list_contains_sha1(output, &expected), Ok(true));
        assert_eq!(pending_list_contains_sha1(b"", &expected), Ok(false));
        assert_eq!(
            pending_list_contains_sha1(
                b"SHA1 Fingerprint: 01:23:45:67:89:AB:CD:EF:10:32:54:76:98:BA:DC:FE:11:22:33:45\n",
                &expected,
            ),
            Ok(false)
        );
    }

    #[test]
    fn pending_mok_parser_rejects_hostile_or_ambiguous_output() {
        let expected = [0_u8; SHA1_FINGERPRINT_BYTES];
        for hostile in [
            b"SHA1 Fingerprint: 00:00\n".as_slice(),
            b"SHA1 Fingerprint:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00\n",
            b"SHA1 Fingerprint: GG:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00\n",
            b"SHA1 Fingerprint: 00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00\n",
            b"SHA1 Fingerprint: 00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00\n",
            b"\xffSHA1 Fingerprint: 00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00\n",
        ] {
            assert!(
                pending_list_contains_sha1(hostile, &expected).is_err(),
                "hostile fixture was accepted: {hostile:?}"
            );
        }

        let duplicate =
            b"SHA1 Fingerprint: 00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00\n\
SHA1 Fingerprint: 00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00\n";
        assert!(pending_list_contains_sha1(duplicate, &expected).is_err());
        assert!(
            pending_list_contains_sha1(&vec![b'A'; MOK_OUTPUT_LIMIT_BYTES], &expected).is_err()
        );
    }

    #[test]
    fn certificate_fingerprint_is_computed_in_process_and_bounded() {
        assert_eq!(
            certificate_sha1(b"abc").expect("bounded DER fixture hashes"),
            [
                0xa9, 0x99, 0x3e, 0x36, 0x47, 0x06, 0x81, 0x6a, 0xba, 0x3e, 0x25, 0x71, 0x78, 0x50,
                0xc2, 0x6c, 0x9c, 0xd0, 0xd8, 0x9d,
            ]
        );
        assert!(certificate_sha1(b"").is_err());
        assert!(certificate_sha1(&vec![0_u8; MOK_OUTPUT_LIMIT_BYTES]).is_err());
    }

    #[test]
    fn wrong_arm_token_does_not_reboot_it_stages() {
        let fake = FakeActions {
            secure_boot: Some(SecureBootState::Enabled),
            enrolled: false,
            ..Default::default()
        };
        let r = run_enable(&fake, &detect_of("Surface Pro 6"), Some("nope"));
        assert!(matches!(r.mok, MokEnrollment::ImportedAwaitingArm { .. }));
    }

    #[test]
    fn sb_on_enrolled_verifies_modules_load() {
        let fake = FakeActions {
            secure_boot: Some(SecureBootState::Enabled),
            enrolled: true,
            modules_loaded: true,
            ..Default::default()
        };
        let r = run_enable(&fake, &detect_of("Surface Pro 6"), None);
        assert_eq!(
            r.mok,
            MokEnrollment::Enrolled {
                modules_loaded: true
            }
        );
    }

    #[test]
    fn sb_on_enrolled_but_modules_blocked_is_honest_degraded() {
        let fake = FakeActions {
            secure_boot: Some(SecureBootState::Enabled),
            enrolled: true,
            modules_loaded: false,
            ..Default::default()
        };
        let r = run_enable(&fake, &detect_of("Surface Pro 6"), None);
        assert_eq!(
            r.mok,
            MokEnrollment::Enrolled {
                modules_loaded: false
            }
        );
    }

    #[test]
    fn a_gated_sb_read_yields_undetermined_not_a_guess() {
        let fake = FakeActions {
            sb_read_fails: true,
            ..Default::default()
        };
        let r = run_enable(&fake, &detect_of("Surface Pro 6"), None);
        assert!(matches!(r.mok, MokEnrollment::Undetermined { .. }));
    }

    #[test]
    fn a_failed_unit_is_recorded_as_failed_not_dropped() {
        let fake = FakeActions {
            secure_boot: Some(SecureBootState::Disabled),
            enable_fails: true,
            ..Default::default()
        };
        let r = run_enable(&fake, &detect_of("Surface Pro 6"), None);
        assert!(matches!(
            r.activation.units[0].outcome,
            StepOutcome::Failed { .. }
        ));
    }

    #[test]
    fn live_seam_rejects_non_allowlisted_targets_before_effects() {
        let unit = LiveSurfaceActions.enable_unit("caller-controlled.service");
        assert!(matches!(
            unit,
            Err(EnableError::Failed { action, .. }) if action == "activate iptsd"
        ));
        let config = LiveSurfaceActions.apply_config(ConfigKey::RotationHint, "auto");
        assert!(matches!(config, Err(EnableError::Failed { .. })));
        assert!(matches!(
            LiveSurfaceActions.enable_unit(IPTSD_UNIT),
            Err(EnableError::IntegrationGated { action }) if action.contains("Preview / Commit / Cancel / Audit")
        ));
        assert!(matches!(
            LiveSurfaceActions.apply_config(ConfigKey::SamPerfProfile, "balanced"),
            Err(EnableError::IntegrationGated { action }) if action.contains("Preview / Commit / Cancel / Audit")
        ));
        let pending = LiveSurfaceActions.mok_pending(Path::new("/tmp/caller.cer"));
        assert!(matches!(pending, Err(EnableError::Failed { .. })));
        let hostile_pending = LiveSurfaceActions.mok_pending(Path::new("/tmp/cert.cer\n--root-pw"));
        assert!(matches!(hostile_pending, Err(EnableError::Failed { .. })));

        let hostile_import = LiveSurfaceActions.mok_import(Path::new("--root-pw"));
        assert!(matches!(
            hostile_import,
            Err(EnableError::Failed { action, detail })
                if action == "stage MOK key" && detail.contains("not allowlisted")
        ));
        let fixed_import = LiveSurfaceActions.mok_import(Path::new(MOK_KEY_PATH));
        assert!(matches!(
            fixed_import,
            Err(EnableError::IntegrationGated { action })
                if action.contains("sealed one-time credential broker")
                    && action.contains("no EFI mutation")
        ));
    }
}
