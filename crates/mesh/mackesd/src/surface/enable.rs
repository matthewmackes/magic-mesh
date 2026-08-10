//! SURFACE-3 — the `surface_enable` worker + guided MOK enrollment.
//!
//! The day-2 *activation* half of the Microsoft Surface enablement epic
//! (design: `docs/design/surface-tablet-enablement.md`, locks #4 + #6).
//! SURFACE-2's [`crate::surface`] detection folds the DMI identity into a
//! per-model [`SurfaceProfile`]; this unit turns that profile into the
//! observed enablement posture the bootc image *can't* fully prove ahead of time:
//!
//! * **Activate + configure** — after the worker consumes the shared exact-body,
//!   single-use action capability, retrigger the package-owned iptsd udev rule
//!   and apply the fixed Surface platform profile through `surface-control`.
//! * **Guided MOK enrollment** (lock #6) — on a Secure-Boot host whose
//!   linux-surface modules are blocked, stage the machine-owner key
//!   (`mokutil --import`), hand back the **exact blue MOK-Manager firmware
//!   copy** the operator will see, then hand reboot navigation to the shell's
//!   governed host-state workflow (never an auto-reboot). After the reboot a fresh
//!   enable call re-classifies the state as [`MokState::Enrolled`] and
//!   verifies the modules load.
//!
//! **Everything that touches the machine sits behind the injectable
//! [`SurfaceActions`] seam.** The production seam ([`LiveSurfaceActions`])
//! uses fixed binaries, fixed paths, allowlisted arguments, and bounded
//! subprocesses for activation and classification. MOK import uses a
//! request-bound sealed systemd credential and proves the
//! fixed certificate is pending afterward.
//! Pending enrollment is proven in-process by
//! matching the fixed package certificate's complete SHA-1 fingerprint against
//! `mokutil --list-new`; SHA-1 is used only because that is the exact identifier
//! mokutil exposes for this read, not as a trust primitive. Reboot stays behind
//! the host-state worker's exact-body, single-use `propose` then `confirm`
//! authority; the Surface worker cannot safely mint or retain either reboot
//! capability. The pure core —
//! the per-model [`plan_enable`], the
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

#[cfg(feature = "async-services")]
use mackes_mesh_types::surface_enable::{
    SurfaceEnableActivation as SharedActivation, SurfaceEnableConfig as SharedConfig,
    SurfaceEnableConfigResult as SharedConfigResult, SurfaceEnableMokState as SharedMokState,
    SurfaceEnableOutcome as SharedOutcome, SurfaceEnableRefusal as SharedRefusal,
    SurfaceEnableResult as SharedEnableResult, SurfaceEnableSource as SharedSource,
    SurfaceEnableStepOutcome as SharedStepOutcome, SurfaceEnableUnit as SharedUnit,
    SurfaceEnableUnitResult as SharedUnitResult, SURFACE_ENABLE_RESULT_SCHEMA_VERSION,
};
use mackes_mesh_types::surface_hardware::SurfaceProGeneration;

use super::{Subsystem, SurfaceDetection, SurfaceDevice, SurfaceModel, SurfaceProfile};

#[cfg(feature = "async-services")]
#[path = "mok_credential.rs"]
mod mok_credential;

// ─────────────────────────────── constants ──────────────────────────────────

/// The packaged per-device iptsd systemd template. A udev rule owns instance
/// selection; callers can never provide a unit or hidraw path.
pub const IPTSD_UNIT: &str = "iptsd@.service";

#[cfg(feature = "async-services")]
const IPTSD_ACTIVE_PATTERN: &str = "iptsd@*.service";
#[cfg(feature = "async-services")]
const SAM_BALANCED_PROFILE: &str = "balanced";

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
/// performs (systemd, sysfs/config writes, and mokutil). Tests hand a
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
/// fixed, bounded host operations. MOK staging additionally requires a sealed
/// one-use permit bound to the already-authorized request. It deliberately has
/// no reboot action: reboot authority belongs exclusively to host-state.
#[derive(Debug, Clone, Default)]
pub struct LiveSurfaceActions {
    #[cfg(feature = "async-services")]
    mok_binding: Option<mok_credential::MokImportBinding>,
}

impl LiveSurfaceActions {
    #[cfg(feature = "async-services")]
    fn for_request(
        node: &str,
        request_id: &str,
        authorization_nonce: &str,
        authorization_expires_at_ms: u64,
    ) -> Self {
        Self {
            mok_binding: Some(mok_credential::MokImportBinding {
                node: node.to_string(),
                request_id: request_id.to_string(),
                authorization_nonce: authorization_nonce.to_string(),
                authorization_expires_at_ms,
            }),
        }
    }

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

fn validate_unit_target(unit: &str) -> Result<(), EnableError> {
    if unit == IPTSD_UNIT {
        Ok(())
    } else {
        LiveSurfaceActions::failed("activate iptsd", "unit is not the fixed iptsd template")
    }
}

fn validate_config_target(key: ConfigKey, value: &str) -> Result<(), EnableError> {
    match (key, value) {
        (ConfigKey::SamPerfProfile, "balanced") => Ok(()),
        (ConfigKey::SamPerfProfile, _) => LiveSurfaceActions::failed(
            "apply sam_perf_profile",
            "profile is not the fixed balanced Surface profile",
        ),
        _ => LiveSurfaceActions::failed(
            format!("apply {}", key.id()),
            "configuration is package- or DRM-runner-owned",
        ),
    }
}

#[cfg(feature = "async-services")]
fn run_fixed(
    action: &str,
    program: &str,
    args: &[&str],
) -> Result<std::process::Output, EnableError> {
    let mut command = Command::new(program);
    command
        .args(args)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin")
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("SYSTEMD_COLORS", "0");
    crate::workers::proc::output_with_timeout(command, SURFACE_COMMAND_TIMEOUT).map_err(|error| {
        EnableError::Failed {
            action: action.to_string(),
            detail: error.to_string(),
        }
    })
}

#[cfg(feature = "async-services")]
fn command_detail(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    if detail.is_empty() {
        format!("command exited with {}", output.status)
    } else {
        detail.to_string()
    }
}

#[cfg(feature = "async-services")]
fn iptsd_is_active() -> Result<bool, EnableError> {
    let output = run_fixed(
        "query iptsd activation",
        "/usr/bin/systemctl",
        &["is-active", IPTSD_ACTIVE_PATTERN],
    )?;
    Ok(output.status.success())
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

fn format_sha1_fingerprint(fingerprint: &[u8; SHA1_FINGERPRINT_BYTES]) -> String {
    fingerprint
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

#[cfg(feature = "async-services")]
fn wall_now_ms_u64() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
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
        validate_unit_target(unit)?;
        #[cfg(feature = "async-services")]
        {
            if iptsd_is_active()? {
                return Ok(true);
            }

            // iptsd 3.1.0's packaged rule matches hidraw/add, then admits only
            // devices accepted by its fixed `iptsd-check-device --quiet
            // $DEVNAME` helper. No narrower stable property is part of that
            // package contract, so replay the rule's exact subsystem/action
            // scope and let the packaged helper select the device. This worker
            // never accepts a unit instance or /dev path from the request.
            let trigger = run_fixed(
                "activate iptsd",
                "/usr/bin/udevadm",
                &["trigger", "--action=add", "--subsystem-match=hidraw"],
            )?;
            if !trigger.status.success() {
                return Self::failed("activate iptsd", command_detail(&trigger));
            }
            let settle = run_fixed(
                "wait for iptsd activation",
                "/usr/bin/udevadm",
                &["settle", "--timeout=8"],
            )?;
            if !settle.status.success() {
                return Self::failed("wait for iptsd activation", command_detail(&settle));
            }
            if !iptsd_is_active()? {
                return Self::failed(
                    "activate iptsd",
                    "the package udev rule did not start an iptsd device instance",
                );
            }
            return Ok(false);
        }
        #[cfg(not(feature = "async-services"))]
        Self::gated("activate iptsd (async-services disabled)")
    }

    fn apply_config(&self, key: ConfigKey, value: &str) -> Result<(), EnableError> {
        validate_config_target(key, value)?;
        #[cfg(feature = "async-services")]
        {
            // surface-control validates the requested profile against the
            // kernel-advertised choices and skips the write when it is already
            // current. Both the executable and every argument are fixed here.
            let apply = run_fixed(
                "apply sam_perf_profile",
                "/usr/bin/surface",
                &["--quiet", "profile", "set", SAM_BALANCED_PROFILE],
            )?;
            if !apply.status.success() {
                return Self::failed("apply sam_perf_profile", command_detail(&apply));
            }
            let verify = run_fixed(
                "verify sam_perf_profile",
                "/usr/bin/surface",
                &["--quiet", "profile", "get"],
            )?;
            if !verify.status.success() {
                return Self::failed("verify sam_perf_profile", command_detail(&verify));
            }
            let current = std::str::from_utf8(&verify.stdout)
                .map_err(|_| EnableError::Failed {
                    action: "verify sam_perf_profile".to_string(),
                    detail: "surface-control returned a non-UTF-8 profile".to_string(),
                })?
                .trim();
            if current != SAM_BALANCED_PROFILE || !verify.stderr.is_empty() {
                return Self::failed(
                    "verify sam_perf_profile",
                    "surface-control did not confirm the fixed balanced profile",
                );
            }
            return Ok(());
        }
        #[cfg(not(feature = "async-services"))]
        Self::gated("apply sam_perf_profile (async-services disabled)")
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
        #[cfg(feature = "async-services")]
        {
            let Some(binding) = self.mok_binding.as_ref() else {
                return Self::gated(
                    "stage MOK key (authorized Surface request binding is unavailable)",
                );
            };
            let metadata =
                std::fs::symlink_metadata(MOK_KEY_PATH).map_err(|error| EnableError::Failed {
                    action: "stage MOK key".to_string(),
                    detail: format!("Surface certificate is unavailable: {error}"),
                })?;
            if !metadata.file_type().is_file()
                || metadata.len() == 0
                || metadata.len() > MOK_OUTPUT_LIMIT_BYTES as u64
            {
                return Self::failed(
                    "stage MOK key",
                    "Surface certificate is not a bounded regular file",
                );
            }
            let certificate = mok_credential::read_bounded_regular(
                Path::new(MOK_KEY_PATH),
                MOK_OUTPUT_LIMIT_BYTES - 1,
            )
            .map_err(|error| EnableError::Failed {
                action: "stage MOK key".to_string(),
                detail: format!("read fixed Surface certificate: {error}"),
            })?;
            let fingerprint =
                certificate_sha1(&certificate).map_err(|detail| EnableError::Failed {
                    action: "stage MOK key".to_string(),
                    detail: detail.to_string(),
                })?;
            let permit =
                mok_credential::load_systemd_permit(binding, &fingerprint, wall_now_ms_u64())
                    .map_err(|detail| EnableError::Failed {
                        action: "stage MOK key".to_string(),
                        detail,
                    })?;
            mok_credential::import_fixed_certificate(permit.password()).map_err(|detail| {
                EnableError::Failed {
                    action: "stage MOK key".to_string(),
                    detail,
                }
            })?;
            if !self.mok_pending(key_path)? {
                return Self::failed(
                    "stage MOK key",
                    "mokutil returned success but the fixed certificate is not pending",
                );
            }
            return Ok(format_sha1_fingerprint(&fingerprint));
        }
        #[cfg(not(feature = "async-services"))]
        Self::gated("stage MOK key (async-services disabled)")
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
/// `NotSecureBoot → Skip`; `KeyMissing → ImportThenAwaitHostReboot`; `Enrolled →
/// VerifyModules`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MokStep {
    /// Secure Boot off — skip MOK entirely.
    Skip,
    /// Stage the key, then await the separately governed host reboot workflow.
    ImportThenAwaitHostReboot,
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
        MokState::KeyMissing => MokStep::ImportThenAwaitHostReboot,
        MokState::Enrolled => MokStep::VerifyModules,
    }
}

/// Exact typed-confirmation equality helper shared by Surface action flows.
/// This helper carries no authority and has no MOK/reboot semantics; each
/// caller remains responsible for its own governed action contract.
#[must_use]
pub(crate) fn is_armed(provided: Option<&str>, expected: &str) -> bool {
    provided.is_some_and(|value| value == expected)
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
still staged until enrolled. Reboot only through System → Power & Battery."
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
    /// Key staged, awaiting the separately governed host reboot. Carries the
    /// exact firmware copy + the key fingerprint confirmed at the blue screen.
    ImportedAwaitingArm {
        /// The blue-screen firmware copy ([`mok_firmware_prompt`]).
        firmware_prompt: String,
        /// Reserved wire-compatibility field. New producers always emit an
        /// empty string; reboot authority is never carried in this result.
        arm_token: String,
        /// The staged key's fingerprint (confirmed at the blue screen).
        key_fingerprint: String,
    },
    /// Legacy read compatibility for results produced before reboot authority
    /// moved exclusively to host-state. New code never produces this variant.
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
    /// Private typed refusal provenance used only at the shared publication
    /// boundary. It is never serialized as part of this legacy diagnostic.
    #[serde(skip)]
    refusal: Option<EnableRefusal>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnableRefusal {
    Contract,
    Authorization,
    ObsoleteRebootArm,
    Policy,
}

impl EnableResult {
    /// A skip result carrying the honest reason (non-Surface / unrecognised).
    fn skipped(model: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            skipped: Some(reason.into()),
            activation: ActivationResult::default(),
            mok: MokEnrollment::NotRequired,
            refusal: Some(EnableRefusal::Policy),
        }
    }

    fn refused(
        model: impl Into<String>,
        refusal: EnableRefusal,
        reason: impl Into<String>,
    ) -> Self {
        let mut result = Self::skipped(model, reason);
        result.refusal = Some(refusal);
        result
    }
}

#[cfg(feature = "async-services")]
fn shared_result(
    node: &str,
    request_id: &str,
    published_at_ms: u64,
    device: &SurfaceDevice,
    result: &EnableResult,
) -> Result<SharedEnableResult, &'static str> {
    if !matches!(
        device.contract_generation,
        SurfaceProGeneration::Pro5 | SurfaceProGeneration::Pro6
    ) {
        return Err("result model is outside the exact Surface Pro 5/6 contract");
    }
    let outcome = if let Some(reason) = result.skipped.as_ref() {
        let code = match result.refusal.unwrap_or(EnableRefusal::Policy) {
            EnableRefusal::Contract => SharedRefusal::Contract,
            EnableRefusal::Authorization => SharedRefusal::Authorization,
            EnableRefusal::ObsoleteRebootArm => SharedRefusal::ObsoleteRebootArm,
            EnableRefusal::Policy => SharedRefusal::Policy,
        };
        SharedOutcome::Refused {
            code,
            reason: reason.clone(),
        }
    } else {
        let units = result
            .activation
            .units
            .iter()
            .map(|row| {
                if row.unit != IPTSD_UNIT {
                    return Err("unrecognized enable unit");
                }
                Ok(SharedUnitResult {
                    unit: SharedUnit::Iptsd,
                    outcome: shared_step_outcome(&row.outcome),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let configs = result
            .activation
            .configs
            .iter()
            .map(|row| {
                if row.key != ConfigKey::SamPerfProfile {
                    return Err("unrecognized enable config");
                }
                Ok(SharedConfigResult {
                    config: SharedConfig::SamBalancedProfile,
                    outcome: shared_step_outcome(&row.outcome),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mok = match &result.mok {
            MokEnrollment::NotRequired => SharedMokState::NotRequired,
            MokEnrollment::Enrolled { modules_loaded } => SharedMokState::Enrolled {
                modules_loaded: *modules_loaded,
            },
            MokEnrollment::ImportedAwaitingArm {
                firmware_prompt,
                key_fingerprint,
                ..
            } => SharedMokState::AwaitingGovernedHostReboot {
                firmware_prompt: firmware_prompt.clone(),
                key_fingerprint: key_fingerprint.clone(),
            },
            MokEnrollment::Undetermined { reason } => SharedMokState::Undetermined {
                reason: reason.clone(),
            },
            MokEnrollment::RebootArmed { .. } => {
                return Err("legacy reboot state cannot enter the shared contract");
            }
        };
        SharedOutcome::Completed {
            activation: SharedActivation { units, configs },
            mok,
        }
    };
    let shared = SharedEnableResult {
        schema_version: SURFACE_ENABLE_RESULT_SCHEMA_VERSION,
        node: node.to_string(),
        request_id: request_id.to_string(),
        model: device.product.clone(),
        generation: device.contract_generation,
        source: SharedSource::LocalSurfaceEnableWorker,
        published_at_ms,
        outcome,
    };
    shared
        .validate()
        .map_err(|_| "shared result validation failed")?;
    Ok(shared)
}

#[cfg(feature = "async-services")]
fn shared_step_outcome(outcome: &StepOutcome) -> SharedStepOutcome {
    match outcome {
        StepOutcome::Applied => SharedStepOutcome::Applied,
        StepOutcome::AlreadyActive => SharedStepOutcome::AlreadyActive,
        StepOutcome::Gated { reason } => SharedStepOutcome::Gated {
            reason: reason.clone(),
        },
        StepOutcome::Failed { reason } => SharedStepOutcome::Failed {
            reason: reason.clone(),
        },
    }
}

// ─────────────────────────── the verb (fold over the seam) ──────────────────

/// The `surface_enable` verb: activate + configure this node per its detected
/// model, then walk the guided MOK state machine. Pure control flow over the
/// injectable [`SurfaceActions`] seam — the whole thing is unit-tested with a
/// fake; production hands [`LiveSurfaceActions`] (fixed/bounded reads,
/// request-bound MOK import, and fail-closed staged actions).
///
/// `arm` is retained only for shared request compatibility. Any value is an
/// obsolete reboot-arm request and is refused before activation or MOK effects.
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

    if arm.is_some() {
        return EnableResult::refused(
            device.product.clone(),
            EnableRefusal::ObsoleteRebootArm,
            "obsolete Surface reboot-arm input refused; use the governed System Power & Battery workflow",
        );
    }

    let activation = run_activation(actions, device);
    let mok = run_mok(actions);

    EnableResult {
        model: device.product.clone(),
        skipped: None,
        activation,
        mok,
        refusal: None,
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
fn run_mok(actions: &impl SurfaceActions) -> MokEnrollment {
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
        MokStep::ImportThenAwaitHostReboot => {
            // Stage the key and hand back proof/copy only. Reboot is not part
            // of this seam and can only be proposed/confirmed through host-state.
            match actions.mok_import(Path::new(MOK_KEY_PATH)) {
                Ok(key_fingerprint) => MokEnrollment::ImportedAwaitingArm {
                    firmware_prompt: mok_firmware_prompt(),
                    arm_token: String::new(),
                    key_fingerprint,
                },
                Err(e) => MokEnrollment::Undetermined {
                    reason: e.to_string(),
                },
            }
        }
    }
}

// ─────────────────────────── the Bus worker (per-node) ──────────────────────

#[cfg(feature = "async-services")]
pub use worker::{
    enable_topic, result_topic, EnableRequest, SurfaceEnableWorker, ENABLE_ACTION_AUTH_VERB,
};

#[cfg(feature = "async-services")]
mod worker {
    //! The per-node `surface_enable` Bus worker (a *leader-of-self* worker:
    //! it acts only on its own hardware, never a remote node). It drains
    //! [`enable_topic`] for this node, runs [`super::run_enable`] against the
    //! fail-closed [`super::LiveSurfaceActions`], and publishes the
    //! typed [`super::EnableResult`] to [`result_topic`]. SURFACE-4 folds
    //! that into the fleet enablement summary.

    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    pub use mackes_mesh_types::surface_hardware::SurfaceEnableRequest as EnableRequest;
    use mde_bus::hooks::config::Priority;
    use mde_bus::persist::Persist;

    use super::{
        run_enable, shared_result, EnableRefusal, EnableResult, LiveSurfaceActions, SurfaceModel,
    };
    use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};
    use crate::surface::{detect, SurfaceDetection};
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
                let received_at_ms = wall_now_ms();
                let admitted_request_id = msg.body.as_deref().and_then(|body| {
                    EnableRequest::from_json_at(body.as_bytes(), &self.node_id, received_at_ms)
                        .ok()
                        .map(|request| request.header.request_id)
                });
                let result = self.apply_request(msg.body.as_deref());
                // Publication freshness starts after every activation/MOK
                // effect completes, never when the request was received.
                let published_at_ms = wall_now_ms();
                self.publish(
                    persist,
                    admitted_request_id.as_deref(),
                    published_at_ms,
                    &result,
                );
            }
        }

        /// Authenticate and decode one raw Bus request, then run the typed
        /// enable verb. Parsing is side-effect free; the shared exact-body
        /// gate runs before [`run_enable`] or any privileged seam call.
        fn apply_request(&self, body: Option<&str>) -> EnableResult {
            let Some(body) = body else {
                return self
                    .refused_result(EnableRefusal::Contract, "enable request body is missing");
            };
            let req =
                match EnableRequest::from_json_at(body.as_bytes(), &self.node_id, wall_now_ms()) {
                    Ok(req) => req,
                    Err(error) => {
                        return self.refused_result(
                            EnableRefusal::Contract,
                            &format!("enable request failed shared contract admission: {error}"),
                        );
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
                return self.refused_result(
                    EnableRefusal::Authorization,
                    &format!("surface enable authorization refused: {error}"),
                );
            }
            if req.arm_token.is_some() {
                return self.refused_result(
                    EnableRefusal::ObsoleteRebootArm,
                    "obsolete Surface reboot-arm input refused; use the governed System Power & Battery workflow",
                );
            }
            let Some(authorization) = req
                .header
                .armed_token
                .as_deref()
                .and_then(mackes_mesh_types::cloud::CloudArmedToken::parse)
            else {
                return self.refused_result(
                    EnableRefusal::Authorization,
                    "surface enable authorization refused: verified capability is unavailable",
                );
            };
            let actions = LiveSurfaceActions::for_request(
                &self.node_id,
                &req.header.request_id,
                &authorization.nonce,
                u64::try_from(authorization.expires_at_ms).unwrap_or(0),
            );
            run_enable(&actions, &self.detection, req.arm_token.as_deref())
        }

        fn refused_result(&self, refusal: EnableRefusal, reason: &str) -> EnableResult {
            let model = match &self.detection.model {
                SurfaceModel::Known(device) => device.product.clone(),
                SurfaceModel::UnknownSurface { product } => product.clone(),
                SurfaceModel::NotASurface => String::new(),
            };
            EnableResult::refused(model, refusal, reason)
        }

        /// Publish the typed result to the per-node result lane.
        fn publish(
            &self,
            persist: &Persist,
            request_id: Option<&str>,
            published_at_ms: u64,
            result: &EnableResult,
        ) {
            let Some(request_id) = request_id else {
                tracing::warn!(
                    target: "mackesd::surface_enable",
                    node = %self.node_id,
                    "not publishing an enable diagnostic without an admitted request identity"
                );
                return;
            };
            let SurfaceModel::Known(device) = &self.detection.model else {
                return;
            };
            let Ok(shared) =
                shared_result(&self.node_id, request_id, published_at_ms, device, result)
            else {
                tracing::warn!(
                    target: "mackesd::surface_enable",
                    node = %self.node_id,
                    "not publishing an enable diagnostic outside the strict shared contract"
                );
                return;
            };
            let Ok(body) = shared.to_json() else {
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
        use crate::ipc::action_auth::{authorize_test_body, ActionAuthorizer, MutationContext};
        use crate::surface::{identify, DmiInfo, MS_VENDOR};
        use mackes_mesh_types::surface_enable::{
            SurfaceEnableOutcome as SharedOutcome, SurfaceEnableRefusal as SharedRefusal,
            SurfaceEnableResult as SharedEnableResult,
        };

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
        fn unsupported_surface_result_is_not_published_into_pro5_6_contract() {
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
            assert!(out.is_empty());
        }

        #[test]
        fn authorized_obsolete_reboot_arm_is_refused_before_live_actions() {
            let dir = tempfile::tempdir().expect("tempdir");
            let persist = Persist::open(dir.path().to_path_buf()).expect("open bus");
            let mut worker = authorized_worker("node-a", detection("Surface Pro 6"), dir.path());
            let request = signed_request(
                "node-a",
                Some("REBOOT-TO-ENROLL-MOK"),
                "surface-enable-obsolete-reboot",
            );
            persist
                .write(
                    &enable_topic("node-a"),
                    Priority::Default,
                    None,
                    Some(&request),
                )
                .expect("write request");
            let before = wall_now_ms();

            worker.poll_once(&persist);

            let after = wall_now_ms();
            let output = persist
                .list_since(&result_topic("node-a"), None)
                .expect("list result");
            assert_eq!(output.len(), 1);
            let shared = SharedEnableResult::from_json_at(
                output[0].body.as_deref().unwrap().as_bytes(),
                "node-a",
                "surface-enable-obsolete-reboot",
                after,
            )
            .expect("admit shared result");
            assert!((before..=after).contains(&shared.published_at_ms));
            assert!(matches!(
                shared.outcome,
                SharedOutcome::Refused {
                    code: SharedRefusal::ObsoleteRebootArm,
                    ..
                }
            ));
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
                "arm_token": "obsolete-reboot-arm",
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
            assert!(out.is_empty());
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
            assert!(result
                .skipped
                .as_deref()
                .is_some_and(|reason| reason.contains("targets a different node")));
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
            assert!(result
                .skipped
                .as_deref()
                .is_some_and(|reason| reason.contains("stale or future-dated")));
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
            let results: Vec<SharedEnableResult> = out
                .iter()
                .map(|item| {
                    SharedEnableResult::from_json_at(
                        item.body.as_deref().unwrap().as_bytes(),
                        "node-replay",
                        "surface-enable-replay",
                        wall_now_ms(),
                    )
                    .unwrap()
                })
                .collect();
            assert!(matches!(
                results[0].outcome,
                SharedOutcome::Refused {
                    code: SharedRefusal::Authorization,
                    ..
                }
            ));
            assert!(matches!(
                results[1].outcome,
                SharedOutcome::Completed { .. }
            ));
            assert!(matches!(
                results[2].outcome,
                SharedOutcome::Refused {
                    code: SharedRefusal::Authorization,
                    ..
                }
            ));
        }
    }
}

// ─────────────────────────────── tests ──────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::surface::{identify, DmiInfo, SurfaceFamily, MS_VENDOR};

    /// A fake seam whose every action is scripted, so the fold + state
    /// machine run green without touching a machine.
    #[derive(Default)]
    struct FakeActions {
        secure_boot: Option<SecureBootState>,
        enrolled: bool,
        pending: bool,
        modules_loaded: bool,
        import_fingerprint: Option<String>,
        effects: Cell<usize>,
        // failure injection
        enable_fails: bool,
        sb_read_fails: bool,
    }

    impl SurfaceActions for FakeActions {
        fn enable_unit(&self, unit: &str) -> Result<bool, EnableError> {
            self.effects.set(self.effects.get() + 1);
            if self.enable_fails {
                return Err(EnableError::Failed {
                    action: format!("enable {unit}"),
                    detail: "unit masked".into(),
                });
            }
            Ok(false)
        }
        fn apply_config(&self, _key: ConfigKey, _value: &str) -> Result<(), EnableError> {
            self.effects.set(self.effects.get() + 1);
            Ok(())
        }
        fn secure_boot_state(&self) -> Result<SecureBootState, EnableError> {
            self.effects.set(self.effects.get() + 1);
            if self.sb_read_fails {
                return Err(EnableError::IntegrationGated {
                    action: "read secure-boot state".into(),
                });
            }
            Ok(self.secure_boot.unwrap_or(SecureBootState::Disabled))
        }
        fn mok_enrolled(&self) -> Result<bool, EnableError> {
            self.effects.set(self.effects.get() + 1);
            Ok(self.enrolled)
        }
        fn mok_pending(&self, _key_path: &Path) -> Result<bool, EnableError> {
            self.effects.set(self.effects.get() + 1);
            Ok(self.pending)
        }
        fn mok_import(&self, _key_path: &Path) -> Result<String, EnableError> {
            self.effects.set(self.effects.get() + 1);
            Ok(self
                .import_fingerprint
                .clone()
                .unwrap_or_else(|| "AA:BB:CC".into()))
        }
        fn modules_loaded(&self, _modules: &[&str]) -> Result<bool, EnableError> {
            self.effects.set(self.effects.get() + 1);
            Ok(self.modules_loaded)
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
        assert_eq!(
            mok_step(MokState::KeyMissing),
            MokStep::ImportThenAwaitHostReboot
        );
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
    fn firmware_prompt_names_enroll_mok_and_governed_reboot_destination() {
        let copy = mok_firmware_prompt();
        assert!(copy.contains("Enroll MOK"));
        assert!(copy.contains("one-time password"));
        assert!(copy.contains("System → Power & Battery"));
        assert!(!copy.contains("REBOOT-TO-ENROLL-MOK"));
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
        assert!(r
            .skipped
            .as_deref()
            .unwrap()
            .contains("unrecognised Surface"));
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
            Some("obsolete-reboot-arm"),
        );
        assert!(result
            .skipped
            .as_deref()
            .is_some_and(|reason| reason.contains("Pro 5/6 action contract")));
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
        assert!(r
            .activation
            .configs
            .iter()
            .all(|c| c.outcome == StepOutcome::Applied));
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
                assert!(arm_token.is_empty());
                assert_eq!(key_fingerprint, "12:34:56");
                assert!(firmware_prompt.contains("Enroll MOK"));
            }
            other => panic!("expected ImportedAwaitingArm, got {other:?}"),
        }
    }

    #[test]
    fn obsolete_reboot_arm_is_refused_before_every_effect() {
        let fake = FakeActions {
            secure_boot: Some(SecureBootState::Enabled),
            enrolled: false,
            pending: true,
            ..Default::default()
        };
        let result = run_enable(
            &fake,
            &detect_of("Surface Pro 6"),
            Some("REBOOT-TO-ENROLL-MOK"),
        );
        assert_eq!(fake.effects.get(), 0);
        assert!(result.activation.units.is_empty());
        assert!(result.activation.configs.is_empty());
        assert_eq!(result.mok, MokEnrollment::NotRequired);
        assert!(result
            .skipped
            .as_deref()
            .is_some_and(|reason| reason.contains("obsolete Surface reboot-arm input refused")));
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
    fn every_non_null_arm_value_is_refused_instead_of_staging() {
        let fake = FakeActions {
            secure_boot: Some(SecureBootState::Enabled),
            enrolled: false,
            ..Default::default()
        };
        let r = run_enable(&fake, &detect_of("Surface Pro 6"), Some("nope"));
        assert_eq!(fake.effects.get(), 0);
        assert!(r.skipped.is_some());
    }

    #[cfg(feature = "async-services")]
    #[test]
    fn shared_projection_removes_legacy_reboot_state_and_binds_request() {
        let internal = EnableResult {
            model: "Surface Pro 6".into(),
            skipped: None,
            activation: ActivationResult {
                units: vec![UnitResult {
                    unit: IPTSD_UNIT.into(),
                    outcome: StepOutcome::Applied,
                }],
                configs: vec![ConfigResult {
                    key: ConfigKey::SamPerfProfile,
                    subsystem: Subsystem::Sam,
                    outcome: StepOutcome::AlreadyActive,
                }],
            },
            mok: MokEnrollment::ImportedAwaitingArm {
                firmware_prompt: mok_firmware_prompt(),
                arm_token: String::new(),
                key_fingerprint: "01:23:45:67:89:AB:CD:EF:10:32:54:76:98:BA:DC:FE:11:22:33:44"
                    .into(),
            },
            refusal: None,
        };
        let shared = shared_result(
            "surface-6",
            "surface-enable-request",
            1_800_000_000_000,
            &device_of("Surface Pro 6"),
            &internal,
        )
        .expect("project strict shared result");
        let body = shared.to_json().expect("encode strict shared result");

        assert!(!body.contains("arm_token"));
        assert!(!body.contains("RebootArmed"));
        assert!(matches!(
            shared.outcome,
            SharedOutcome::Completed {
                mok: SharedMokState::AwaitingGovernedHostReboot { .. },
                ..
            }
        ));
    }

    #[cfg(feature = "async-services")]
    #[test]
    fn shared_refusal_codes_are_typed_not_inferred_from_prose() {
        for (internal, expected) in [
            (EnableRefusal::Contract, SharedRefusal::Contract),
            (EnableRefusal::Authorization, SharedRefusal::Authorization),
            (
                EnableRefusal::ObsoleteRebootArm,
                SharedRefusal::ObsoleteRebootArm,
            ),
            (EnableRefusal::Policy, SharedRefusal::Policy),
        ] {
            let diagnostic = EnableResult::refused(
                "Surface Pro 6",
                internal,
                "identical wording for every typed refusal",
            );
            let shared = shared_result(
                "surface-6",
                "surface-enable-refusal",
                1_800_000_000_000,
                &device_of("Surface Pro 6"),
                &diagnostic,
            )
            .expect("project typed refusal");
            assert!(matches!(
                shared.outcome,
                SharedOutcome::Refused { code, .. } if code == expected
            ));
        }
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
        let actions = LiveSurfaceActions::default();
        let unit = actions.enable_unit("caller-controlled.service");
        assert!(matches!(
            unit,
            Err(EnableError::Failed { action, .. }) if action == "activate iptsd"
        ));
        let config = actions.apply_config(ConfigKey::RotationHint, "auto");
        assert!(matches!(config, Err(EnableError::Failed { .. })));
        assert!(matches!(
            validate_config_target(ConfigKey::SamPerfProfile, "performance"),
            Err(EnableError::Failed { action, detail })
                if action == "apply sam_perf_profile" && detail.contains("fixed balanced")
        ));
        assert_eq!(validate_unit_target(IPTSD_UNIT), Ok(()));
        assert_eq!(
            validate_config_target(ConfigKey::SamPerfProfile, "balanced"),
            Ok(())
        );
        let pending = actions.mok_pending(Path::new("/tmp/caller.cer"));
        assert!(matches!(pending, Err(EnableError::Failed { .. })));
        let hostile_pending = actions.mok_pending(Path::new("/tmp/cert.cer\n--root-pw"));
        assert!(matches!(hostile_pending, Err(EnableError::Failed { .. })));

        let hostile_import = actions.mok_import(Path::new("--root-pw"));
        assert!(matches!(
            hostile_import,
            Err(EnableError::Failed { action, detail })
                if action == "stage MOK key" && detail.contains("not allowlisted")
        ));
        let fixed_import = actions.mok_import(Path::new(MOK_KEY_PATH));
        assert!(matches!(
            fixed_import,
            Err(EnableError::IntegrationGated { action })
                if action.contains("authorized Surface request binding")
        ));
    }
}
