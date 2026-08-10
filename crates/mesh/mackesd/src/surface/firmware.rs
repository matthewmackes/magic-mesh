//! SURFACE-5 — the fwupd/LVFS firmware panel (list + typed-armed apply).
//!
//! The *updates* half of the Microsoft Surface enablement epic (design:
//! `docs/design/surface-tablet-enablement.md`, lock #8). A bootc image swap
//! carries the linux-surface enablement forward, but the **device firmware**
//! (UEFI/system firmware, the touch controller, the Surface Aggregator, the
//! UEFI dbx revocation list, …) still updates out-of-band through fwupd/LVFS.
//! This unit is the mackesd half:
//!
//! * **List** the node's updatable firmware components via fwupd
//!   (`fwupdmgr get-devices` for the inventory + `get-updates` for what has a
//!   newer release): device id, name, current version, the available version
//!   when one exists, and whether that constitutes an update.
//! * A **typed-armed apply** verb ([`run_apply`]) that runs the fwupd update
//!   for a chosen device only when the operator types the exact
//!   [`FW_ARM_TOKEN`] — the same interlock SURFACE-3's MOK reboot uses. An
//!   un-armed apply is *refused*, never auto-applied.
//! * **Verify re-runs after** a successful apply — the apply worker reuses
//!   SURFACE-4's [`crate::surface::verify::run_verify`] hook and re-publishes
//!   the board + summary so the Test tab reflects the new firmware.
//! * Publishes the firmware **inventory** to
//!   `state/hardware/surface/<node>/firmware` (the Install tab's firmware
//!   panel).
//!
//! **Every fwupd call sits behind the injectable [`Fwupd`] seam.** The JSON
//! parse is a pure fold ([`inventory_from_json`]) unit-tested with fixtures;
//! the production seam ([`LiveFwupd`]) invokes only fixed `/usr/bin/fwupdmgr`
//! read argv with a locale, time, and output bound. Apply remains fail-closed
//! after binding a fresh inventory publication, exact release, and SHA-256:
//! the production exact-release install seam is not implemented yet.
//! §6-clean: it stays wholly in mackesd.

use std::io::{self, Read};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use mackes_mesh_types::surface_hardware::{
    SURFACE_HARDWARE_SCHEMA_VERSION, SurfaceAvailability, SurfaceFirmwareDevice,
    SurfaceFirmwareInventory, SurfaceModelIdentity, SurfaceObservationSource, SurfaceProGeneration,
    SurfacePublication,
};

use super::{SurfaceDetection, SurfaceModel};

/// The exact token the operator must type to arm a firmware apply (lock #8 —
/// never an auto-apply).
///
/// Deliberately unambiguous; the Install tab shows it and the apply request
/// echoes it back in `arm_token`. Mirrors SURFACE-3's
/// [`super::enable::MOK_ARM_TOKEN`] interlock.
pub const FW_ARM_TOKEN: &str = "APPLY-SURFACE-FIRMWARE";

const FWUPDMGR: &str = "/usr/bin/fwupdmgr";
const GET_DEVICES_ARGS: [&str; 3] = ["get-devices", "--json", "--no-unreported-check"];
const GET_UPDATES_ARGS: [&str; 3] = ["get-updates", "--json", "--no-unreported-check"];
const FWUPD_READ_TIMEOUT: Duration = Duration::from_secs(20);
const FWUPD_POLL: Duration = Duration::from_millis(25);
const MAX_FWUPD_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_FWUPD_DEVICES: usize = 64;
const MAX_FWUPD_FIELD_BYTES: usize = 512;

// ─────────────────────────────── the seam ───────────────────────────────────

/// A typed failure from the [`Fwupd`] seam — mirrors
/// [`super::enable::EnableError`]'s honest split.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FwError {
    /// A mutating fwupd action is deliberately unavailable because its shared
    /// authority contract is not strong enough yet.
    IntegrationGated {
        /// The fwupd action that is integration-gated.
        action: String,
    },
    /// The live fwupd call ran and failed for a concrete reason (fwupd
    /// unreachable, JSON malformed, the device rejected the update).
    Failed {
        /// The action that failed.
        action: String,
        /// The underlying reason.
        detail: String,
    },
}

impl std::fmt::Display for FwError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IntegrationGated { action } => {
                write!(f, "{action}: integration-gated (live fwupd/LVFS)")
            }
            Self::Failed { action, detail } => write!(f, "{action}: {detail}"),
        }
    }
}

impl std::error::Error for FwError {}

/// The injectable seam over the fwupd calls the firmware panel needs. Tests
/// hand a fixture-scripted fake; production hands [`LiveFwupd`].
///
/// The two read methods return fwupd's raw `--json` text so the parse stays a
/// pure fold ([`inventory_from_json`]); the apply is a typed verb over a
/// chosen device id (§9 — no raw shell string leaves this module).
///
/// # Errors
///
/// Reads return [`FwError::Failed`] for missing binaries, timeouts, bad status,
/// oversized output, or malformed JSON. Mutation remains integration-gated.
pub trait Fwupd {
    /// Raw `fwupdmgr get-devices --json` output (the full inventory).
    ///
    /// # Errors
    /// The seam's typed [`FwError`] (gated / failed) — see the trait docs.
    fn get_devices_json(&self) -> Result<String, FwError>;
    /// Raw `fwupdmgr get-updates --json` output (only devices with a newer
    /// release, each carrying its available `Releases`).
    ///
    /// # Errors
    /// The seam's typed [`FwError`] (gated / failed) — see the trait docs.
    fn get_updates_json(&self) -> Result<String, FwError>;
    /// Future exact-bound apply seam. Production refuses this until the shared
    /// request includes inventory generation, release, and checksum bindings.
    ///
    /// # Errors
    /// The seam's typed [`FwError`] (gated / failed) — see the trait docs.
    fn apply_update(
        &self,
        device_id: &str,
        release_version: &str,
        release_checksum: &str,
    ) -> Result<(), FwError>;
}

/// The production seam. Reads use fixed, bounded `/usr/bin/fwupdmgr` argv.
#[derive(Debug, Clone, Copy, Default)]
pub struct LiveFwupd;

impl LiveFwupd {
    fn read_json(action: &str, args: &[&str]) -> Result<String, FwError> {
        let output = run_fwupdmgr(args).map_err(|error| FwError::Failed {
            action: action.to_string(),
            detail: error.to_string(),
        })?;
        if !output.status.success() {
            return Err(FwError::Failed {
                action: action.to_string(),
                detail: bounded_text(&output.stderr),
            });
        }
        if output.stdout_truncated {
            return Err(FwError::Failed {
                action: action.to_string(),
                detail: format!("stdout exceeded {MAX_FWUPD_OUTPUT_BYTES} bytes"),
            });
        }
        if output.stderr_truncated {
            return Err(FwError::Failed {
                action: action.to_string(),
                detail: format!("stderr exceeded {MAX_FWUPD_OUTPUT_BYTES} bytes"),
            });
        }
        String::from_utf8(output.stdout).map_err(|error| FwError::Failed {
            action: action.to_string(),
            detail: format!("stdout was not UTF-8: {error}"),
        })
    }
}

impl Fwupd for LiveFwupd {
    fn get_devices_json(&self) -> Result<String, FwError> {
        Self::read_json("fwupdmgr get-devices", &GET_DEVICES_ARGS)
    }
    fn get_updates_json(&self) -> Result<String, FwError> {
        Self::read_json("fwupdmgr get-updates", &GET_UPDATES_ARGS)
    }
    fn apply_update(
        &self,
        _device_id: &str,
        _release_version: &str,
        _release_checksum: &str,
    ) -> Result<(), FwError> {
        Err(FwError::IntegrationGated {
            action: "fwupdmgr exact-release install is not implemented".into(),
        })
    }
}

struct FwupdOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

struct Captured {
    bytes: Vec<u8>,
    truncated: bool,
}

fn read_bounded(mut reader: impl Read) -> io::Result<Captured> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8 * 1024];
    let mut truncated = false;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            return Ok(Captured { bytes, truncated });
        }
        let keep = count.min(MAX_FWUPD_OUTPUT_BYTES.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&buffer[..keep]);
        truncated |= keep != count;
    }
}

fn reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn run_fwupdmgr(args: &[&str]) -> io::Result<FwupdOutput> {
    let mut child = Command::new(FWUPDMGR)
        .args(args)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().ok_or_else(|| {
        reap(&mut child);
        io::Error::other("fwupdmgr stdout pipe unavailable")
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        reap(&mut child);
        io::Error::other("fwupdmgr stderr pipe unavailable")
    })?;
    let stdout_reader = thread::spawn(move || read_bounded(stdout));
    let stderr_reader = thread::spawn(move || read_bounded(stderr));
    let deadline = Instant::now() + FWUPD_READ_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                reap(&mut child);
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(error);
            }
        }
        if Instant::now() >= deadline {
            reap(&mut child);
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "fwupdmgr exceeded 20 second timeout",
            ));
        }
        thread::sleep(FWUPD_POLL);
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| io::Error::other("fwupdmgr stdout reader panicked"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| io::Error::other("fwupdmgr stderr reader panicked"))??;
    Ok(FwupdOutput {
        status,
        stdout: stdout.bytes,
        stderr: stderr.bytes,
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
    })
}

fn bounded_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        "fwupdmgr exited unsuccessfully without diagnostics".into()
    } else {
        trimmed.to_string()
    }
}

// ─────────────────────────── the JSON parse (pure fold) ──────────────────────

/// fwupd's `--json` device envelope (`{"Devices":[…]}`). Missing/extra fields
/// are tolerated (`serde(default)`) — best-effort, like every mackesd probe.
#[derive(Debug, Clone, Default, Deserialize)]
struct RawDeviceList {
    #[serde(default, rename = "Devices")]
    devices: Vec<RawDevice>,
}

/// One raw fwupd device row.
#[derive(Debug, Clone, Default, Deserialize)]
struct RawDevice {
    #[serde(default, rename = "DeviceId")]
    device_id: String,
    #[serde(default, rename = "Name")]
    name: String,
    #[serde(default, rename = "Version")]
    version: String,
    #[serde(default, rename = "Plugin")]
    plugin: String,
    #[serde(default, rename = "Releases")]
    releases: Vec<RawRelease>,
}

/// One raw fwupd release row (a candidate firmware version).
#[derive(Debug, Clone, Default, Deserialize)]
struct RawRelease {
    #[serde(default, rename = "Version")]
    version: String,
    #[serde(default, rename = "Checksum")]
    checksums: Vec<String>,
}

/// One updatable firmware component on the node — the Install tab's row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FwDevice {
    /// fwupd's stable device id (the apply verb targets this).
    pub device_id: String,
    /// Human name (`"System Firmware"`, `"UEFI dbx"`, `"Touch Controller"`).
    pub name: String,
    /// The fwupd plugin that owns the device (`"uefi_capsule"`, `"uefi_dbx"`).
    pub plugin: String,
    /// The currently-installed firmware version.
    pub current_version: String,
    /// The newest available version, when fwupd reports a release for it.
    pub available_version: Option<String>,
    /// SHA-256 of the exact available cabinet, when fwupd publishes one.
    pub available_checksum: Option<String>,
    /// Whether the available version is a genuine update over the current one
    /// (see [`version_newer`]) — the field the panel's "Update" button gates
    /// on. A present-but-not-newer release is honestly *not* an update.
    pub update_available: bool,
}

/// Is `candidate` a newer firmware version than `current`?
///
/// A dotted/dashed numeric compare (`"1.2.10" > "1.2.9"`, `"20240101" >
/// "20230601"`), component-by-component with a missing component read as 0.
/// Pure; the fuzzy tail (non-numeric suffixes) is ignored rather than guessed.
#[must_use]
pub fn version_newer(candidate: &str, current: &str) -> bool {
    let parse = |v: &str| {
        v.split(['.', '-', '_'])
            .filter_map(|p| p.parse::<u64>().ok())
            .collect::<Vec<u64>>()
    };
    let cand = parse(candidate);
    let cur = parse(current);
    for i in 0..cand.len().max(cur.len()) {
        let a = cand.get(i).copied().unwrap_or(0);
        let b = cur.get(i).copied().unwrap_or(0);
        if a != b {
            return a > b;
        }
    }
    false
}

/// Fold fwupd's `get-devices` + `get-updates` JSON into the typed inventory.
///
/// The inventory (current versions, names, plugins) comes from `devices_json`;
/// the available versions come from `updates_json` (fwupd only lists a device
/// there when it has a newer release), matched by device id. A device's
/// `update_available` is set only when the matched release is genuinely newer
/// than the installed version ([`version_newer`]) — never a fake "update".
/// Pure; unit-tested against fwupd JSON fixtures.
///
/// # Errors
///
/// Returns [`FwError::Failed`] when either JSON blob doesn't parse as a fwupd
/// device envelope.
pub fn inventory_from_json(
    devices_json: &str,
    updates_json: &str,
) -> Result<Vec<FwDevice>, FwError> {
    mackes_mesh_types::workloads::reject_duplicate_json_keys(devices_json).map_err(|error| {
        FwError::Failed {
            action: "parse fwupdmgr get-devices".into(),
            detail: error.to_string(),
        }
    })?;
    mackes_mesh_types::workloads::reject_duplicate_json_keys(updates_json).map_err(|error| {
        FwError::Failed {
            action: "parse fwupdmgr get-updates".into(),
            detail: error.to_string(),
        }
    })?;
    let devices: RawDeviceList =
        serde_json::from_str(devices_json).map_err(|e| FwError::Failed {
            action: "parse fwupdmgr get-devices".to_string(),
            detail: e.to_string(),
        })?;
    let updates: RawDeviceList =
        serde_json::from_str(updates_json).map_err(|e| FwError::Failed {
            action: "parse fwupdmgr get-updates".to_string(),
            detail: e.to_string(),
        })?;
    validate_raw_inventory(&devices.devices, "get-devices")?;
    validate_raw_inventory(&updates.devices, "get-updates")?;
    Ok(merge_inventory(&devices.devices, &updates.devices))
}

fn validate_raw_inventory(devices: &[RawDevice], source: &str) -> Result<(), FwError> {
    if devices.len() > MAX_FWUPD_DEVICES {
        return Err(FwError::Failed {
            action: format!("parse fwupdmgr {source}"),
            detail: format!("device count exceeds {MAX_FWUPD_DEVICES}"),
        });
    }
    let mut ids = std::collections::HashSet::new();
    for device in devices {
        let valid = [&device.device_id, &device.name, &device.version]
            .into_iter()
            .all(|value| {
                !value.trim().is_empty()
                    && value.len() <= MAX_FWUPD_FIELD_BYTES
                    && !value.chars().any(char::is_control)
            })
            && (device.plugin.is_empty()
                || (device.plugin.len() <= MAX_FWUPD_FIELD_BYTES
                    && !device.plugin.chars().any(char::is_control)))
            && device.releases.len() <= MAX_FWUPD_DEVICES
            && device.releases.iter().all(|release| {
                !release.version.trim().is_empty()
                    && release.version.len() <= MAX_FWUPD_FIELD_BYTES
                    && !release.version.chars().any(char::is_control)
                    && release.checksums.len() <= 8
                    && release.checksums.iter().all(|checksum| {
                        matches!(checksum.len(), 40 | 64)
                            && checksum.bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
            });
        if !valid || !ids.insert(device.device_id.as_str()) {
            return Err(FwError::Failed {
                action: format!("parse fwupdmgr {source}"),
                detail: "invalid, oversized, or duplicate firmware row".into(),
            });
        }
    }
    Ok(())
}

/// Merge the raw device list with the raw update list (pure). Kept separate
/// from [`inventory_from_json`] so the merge logic is testable without JSON.
fn merge_inventory(devices: &[RawDevice], updates: &[RawDevice]) -> Vec<FwDevice> {
    devices
        .iter()
        .map(|dev| {
            // The available release is the update list's first `Releases`
            // entry for this device id (fwupd lists the newest first).
            let available_release = updates
                .iter()
                .find(|u| u.device_id == dev.device_id)
                .and_then(|u| u.releases.first());
            let available_version = available_release
                .map(|release| release.version.clone())
                .filter(|version| !version.is_empty());
            let available_checksum = available_release.and_then(|release| {
                release
                    .checksums
                    .iter()
                    .find(|checksum| checksum.len() == 64)
                    .map(|checksum| checksum.to_ascii_lowercase())
            });
            let update_available = available_version
                .as_deref()
                .is_some_and(|av| version_newer(av, &dev.version));
            FwDevice {
                device_id: dev.device_id.clone(),
                name: dev.name.clone(),
                plugin: dev.plugin.clone(),
                current_version: dev.version.clone(),
                available_version,
                available_checksum,
                update_available,
            }
        })
        .collect()
}

// ─────────────────────────── the inventory verb (fold over the seam) ────────

/// The node's firmware inventory — the model string plus one row per fwupd
/// device. SURFACE-6's Install tab renders it; it publishes to
/// `state/hardware/surface/<node>/firmware`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirmwareInventory {
    /// The recognised model's product string (empty when the node isn't a
    /// recognised Surface — then `devices` is empty and nothing is read).
    pub model: String,
    /// When set, the inventory was skipped/unavailable and this is the honest
    /// reason (not a Surface, or a gated/failed fwupd read).
    pub skipped: Option<String>,
    /// One row per fwupd device, current + available versions.
    pub devices: Vec<FwDevice>,
}

impl FirmwareInventory {
    fn skipped(model: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            skipped: Some(reason.into()),
            devices: Vec::new(),
        }
    }

    /// The count of devices with a genuine update available — the panel's
    /// badge.
    #[must_use]
    pub fn update_count(&self) -> usize {
        self.devices.iter().filter(|d| d.update_available).count()
    }
}

/// The `surface_firmware` list verb: read this node's fwupd inventory through
/// the seam.
///
/// A non-Surface (or unrecognised-Surface) node is skipped cleanly; a
/// gated/failed fwupd read is an honest `skipped` reason with no faked rows.
#[must_use]
pub fn run_inventory(fwupd: &impl Fwupd, detection: &SurfaceDetection) -> FirmwareInventory {
    let model = match &detection.model {
        SurfaceModel::NotASurface => {
            return FirmwareInventory::skipped("", "not a Microsoft Surface");
        }
        SurfaceModel::UnknownSurface { product } => {
            return FirmwareInventory::skipped(
                product.clone(),
                format!("unrecognised Surface: {product} (no per-model profile)"),
            );
        }
        SurfaceModel::Known(dev) => dev.product.clone(),
    };

    let devices_json = match fwupd.get_devices_json() {
        Ok(j) => j,
        Err(e) => return FirmwareInventory::skipped(model, e.to_string()),
    };
    let updates_json = match fwupd.get_updates_json() {
        Ok(json) => json,
        Err(error) => return FirmwareInventory::skipped(model, error.to_string()),
    };

    match inventory_from_json(&devices_json, &updates_json) {
        Ok(devices) => FirmwareInventory {
            model,
            skipped: None,
            devices,
        },
        Err(e) => FirmwareInventory::skipped(model, e.to_string()),
    }
}

fn shared_inventory(
    node: &str,
    detection: &SurfaceDetection,
    inventory: &FirmwareInventory,
    published_at_ms: u64,
) -> Result<SurfaceFirmwareInventory, mackes_mesh_types::surface_hardware::SurfaceContractError> {
    let generation = match &detection.model {
        SurfaceModel::Known(device) => device.contract_generation,
        SurfaceModel::UnknownSurface { .. } | SurfaceModel::NotASurface => {
            SurfaceProGeneration::Unsupported
        }
    };
    let availability = inventory
        .skipped
        .as_ref()
        .map_or(SurfaceAvailability::Fresh, |reason| {
            SurfaceAvailability::Unavailable {
                reason: reason.clone(),
            }
        });
    let value = SurfaceFirmwareInventory {
        publication: SurfacePublication {
            schema_version: SURFACE_HARDWARE_SCHEMA_VERSION,
            node: node.to_string(),
            model: SurfaceModelIdentity {
                product: inventory.model.clone(),
                generation,
            },
            source: SurfaceObservationSource::Fwupd,
            published_at_ms,
            availability,
        },
        skipped: inventory.skipped.clone(),
        devices: inventory
            .devices
            .iter()
            .map(|device| SurfaceFirmwareDevice {
                device_id: device.device_id.clone(),
                name: device.name.clone(),
                plugin: device.plugin.clone(),
                current_version: device.current_version.clone(),
                available_version: device.available_version.clone(),
                available_checksum: device.available_checksum.clone(),
                update_available: device.update_available,
            })
            .collect(),
    };
    value.validate()?;
    Ok(value)
}

/// An empty fwupd device envelope — the honest fallback when the updates read
/// is unavailable (so the inventory still lists devices as up-to-date).
const EMPTY_DEVICE_LIST: &str = r#"{"Devices":[]}"#;

// ─────────────────────────── the apply verb (typed-armed) ───────────────────

/// The outcome of a firmware apply against the seam.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApplyOutcome {
    /// The apply was **refused** because the typed arm didn't match — the
    /// interlock that makes a firmware apply never automatic (lock #8).
    Refused {
        /// Why it was refused (arm token missing/wrong).
        reason: String,
    },
    /// The update was applied this call.
    Applied,
    /// The live apply is integration-gated (honest, §7).
    Gated {
        /// The gated action's reason string.
        reason: String,
    },
    /// The live apply ran and failed.
    Failed {
        /// The failure reason.
        reason: String,
    },
}

impl ApplyOutcome {
    /// Does this outcome trigger a verify re-run? Only a genuinely
    /// [`Self::Applied`] update changes the firmware, so only it re-runs
    /// SURFACE-4's verify (a refused/gated/failed apply changed nothing). Pure.
    #[must_use]
    pub const fn triggers_reverify(&self) -> bool {
        matches!(self, Self::Applied)
    }
}

/// The typed result the `fw-apply` verb returns — what the worker publishes
/// and SURFACE-6's Install tab renders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyResult {
    /// The recognised model's product string (empty when skipped).
    pub model: String,
    /// When set, the apply was skipped and this is the honest reason (not a
    /// Surface / unrecognised Surface).
    pub skipped: Option<String>,
    /// The device id the apply targeted.
    pub device_id: String,
    /// The apply outcome.
    pub outcome: ApplyOutcome,
    /// Whether this apply triggers a verify re-run (a successful apply does).
    pub reverify: bool,
}

impl ApplyResult {
    fn skipped(model: impl Into<String>, device_id: impl Into<String>, reason: &str) -> Self {
        Self {
            model: model.into(),
            skipped: Some(reason.to_string()),
            device_id: device_id.into(),
            outcome: ApplyOutcome::Refused {
                reason: reason.to_string(),
            },
            reverify: false,
        }
    }
}

/// The `fw-apply` verb: apply the firmware update for `device_id`, but only
/// when the operator typed the exact [`FW_ARM_TOKEN`].
///
/// Reuses SURFACE-3's [`super::enable::is_armed`] interlock. An un-armed call
/// is **refused** and nothing runs (lock #8 — never an auto-apply).
/// The selected inventory publication must still be fresh, and an immediate
/// fwupd re-read must reproduce the exact device, release, and SHA-256 binding
/// before the provider seam is allowed to mutate anything.
#[must_use]
pub fn run_apply(
    fwupd: &impl Fwupd,
    detection: &SurfaceDetection,
    device_id: &str,
    arm: Option<&str>,
    inventory_published_at_ms: u64,
    now_ms: u64,
    release_version: &str,
    release_checksum: &str,
) -> ApplyResult {
    let (model, generation) = match &detection.model {
        SurfaceModel::NotASurface => {
            return ApplyResult::skipped("", device_id, "not a Microsoft Surface");
        }
        SurfaceModel::UnknownSurface { product } => {
            return ApplyResult::skipped(
                product.clone(),
                device_id,
                &format!("unrecognised Surface: {product} (no per-model profile)"),
            );
        }
        SurfaceModel::Known(dev) => (dev.product.clone(), dev.contract_generation),
    };

    if !matches!(
        generation,
        SurfaceProGeneration::Pro5 | SurfaceProGeneration::Pro6
    ) {
        return ApplyResult::skipped(
            model,
            device_id,
            "Surface generation is not admitted by the Pro 5/6 firmware contract",
        );
    }

    // The typed-arm interlock: no matching token → refuse, run nothing.
    if !super::enable::is_armed(arm, FW_ARM_TOKEN) {
        return ApplyResult {
            model,
            skipped: None,
            device_id: device_id.to_string(),
            outcome: ApplyOutcome::Refused {
                reason: format!("firmware apply not armed — type {FW_ARM_TOKEN} to confirm"),
            },
            reverify: false,
        };
    }

    let binding_invalid = inventory_published_at_ms == 0
        || inventory_published_at_ms
            > now_ms.saturating_add(
                mackes_mesh_types::surface_hardware::MAX_SURFACE_ACTION_FUTURE_SKEW_MS,
            )
        || now_ms.saturating_sub(inventory_published_at_ms)
            > mackes_mesh_types::surface_hardware::MAX_SURFACE_STATE_AGE_MS
        || release_version.trim().is_empty()
        || release_checksum.len() != 64
        || !release_checksum
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if binding_invalid {
        return ApplyResult {
            model,
            skipped: None,
            device_id: device_id.to_string(),
            outcome: ApplyOutcome::Refused {
                reason: "firmware apply selection is stale or has an invalid release binding"
                    .into(),
            },
            reverify: false,
        };
    }

    let fresh = run_inventory(fwupd, detection);
    let Some(device) = fresh.devices.iter().find(|device| {
        device.device_id == device_id
            && device.update_available
            && device.available_version.as_deref() == Some(release_version)
            && device.available_checksum.as_deref() == Some(release_checksum)
    }) else {
        return ApplyResult {
            model,
            skipped: None,
            device_id: device_id.to_string(),
            outcome: ApplyOutcome::Refused {
                reason: fresh.skipped.unwrap_or_else(|| {
                    "firmware release changed or no longer matches the selected SHA-256".into()
                }),
            },
            reverify: false,
        };
    };

    let outcome = match fwupd.apply_update(&device.device_id, release_version, release_checksum) {
        Ok(()) => ApplyOutcome::Applied,
        Err(FwError::IntegrationGated { action }) => ApplyOutcome::Gated { reason: action },
        Err(FwError::Failed { action, detail }) => ApplyOutcome::Failed {
            reason: format!("{action}: {detail}"),
        },
    };
    let reverify = outcome.triggers_reverify();

    ApplyResult {
        model,
        skipped: None,
        device_id: device_id.to_string(),
        outcome,
        reverify,
    }
}

// ─────────────────────────── the Bus worker (per-node) ──────────────────────

#[cfg(feature = "async-services")]
pub use worker::{
    FW_ACTION_AUTH_VERB, FwApplyRequest, SurfaceFirmwareWorker, fw_apply_topic, fw_result_topic,
    inventory_topic,
};

#[cfg(feature = "async-services")]
mod worker {
    //! The per-node `surface_firmware` Bus worker (a *leader-of-self* worker:
    //! it reads + updates only its own firmware, never a remote node). Each
    //! tick it publishes the fwupd inventory to [`inventory_topic`]; it drains
    //! [`fw_apply_topic`] for typed-armed apply requests, runs
    //! [`super::run_apply`] against [`super::LiveFwupd`],
    //! publishes the [`super::ApplyResult`] to [`fw_result_topic`], and on a
    //! successful apply re-runs SURFACE-4's verify (reusing
    //! [`crate::surface::verify::run_verify`]) and re-publishes the board +
    //! summary. On a non-Surface node it idles (never touches the Bus).

    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    pub use mackes_mesh_types::surface_hardware::SurfaceFirmwareApplyRequest as FwApplyRequest;
    use mde_bus::hooks::config::Priority;
    use mde_bus::persist::Persist;

    use super::{ApplyResult, LiveFwupd, SurfaceModel, run_apply, run_inventory, shared_inventory};
    use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};
    use crate::surface::verify::{
        LiveSurfaceProbes, board_topic, run_verify, shared_board, shared_summary, summary_topic,
    };
    use crate::surface::{SurfaceDetection, detect};
    use crate::workers::{ShutdownToken, Worker};

    /// Poll cadence — firmware is operator-driven + slow-moving, so a modest
    /// tick keeps the panel fresh without churn.
    pub const POLL: Duration = Duration::from_secs(30);

    /// Closed semantic verb bound into every firmware apply capability.
    ///
    /// This is part of the `fw-apply` wire contract: a publisher must mint an
    /// HMAC v1 capability for this verb, the target node, and the fwupd device
    /// id before it writes the request to the action topic.
    pub const FW_ACTION_AUTH_VERB: &str = "surface-firmware-apply";

    /// The per-node lane the fwupd inventory lands on (the Install tab panel).
    #[must_use]
    pub fn inventory_topic(node: &str) -> String {
        format!("state/hardware/surface/{node}/firmware")
    }

    /// The per-node request lane the Install tab publishes fw-apply requests on.
    #[must_use]
    pub fn fw_apply_topic(node: &str) -> String {
        format!("action/hardware/surface/{node}/fw-apply")
    }

    /// The per-node result lane the typed [`ApplyResult`] lands on.
    #[must_use]
    pub fn fw_result_topic(node: &str) -> String {
        format!("state/hardware/surface/{node}/fw-apply")
    }

    /// The per-node `surface_firmware` worker.
    pub struct SurfaceFirmwareWorker {
        node_id: String,
        detection: SurfaceDetection,
        bus_root: Option<PathBuf>,
        poll: Duration,
        action_cursor: Option<String>,
        authorizer: Arc<ActionAuthorizer>,
    }

    impl SurfaceFirmwareWorker {
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

        /// Test constructor: an explicit detection + bus root, no real fwupd.
        #[cfg(test)]
        #[must_use]
        pub(crate) fn with_parts(
            node_id: String,
            detection: SurfaceDetection,
            bus_root: PathBuf,
        ) -> Self {
            Self::with_parts_and_authorizer(
                node_id,
                detection,
                bus_root,
                Arc::new(ActionAuthorizer::production()),
            )
        }

        /// Test constructor with an injectable shared action authorizer. This
        /// is the mint/verify seam for focused tests; production always uses
        /// [`ActionAuthorizer::production`].
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

        /// Publish the current fwupd inventory. Pulled out so a test drives it
        /// against a temp Bus without the run loop/clock.
        fn publish_inventory(&self, persist: &Persist) {
            let inventory = run_inventory(&LiveFwupd, &self.detection);
            match shared_inventory(&self.node_id, &self.detection, &inventory, wall_now_ms()) {
                Ok(inventory) => publish(persist, &inventory_topic(&self.node_id), &inventory),
                Err(error) => tracing::warn!(
                    target: "mackesd::surface_firmware",
                    node = %self.node_id,
                    %error,
                    "refusing invalid Surface firmware publication"
                ),
            }
        }

        /// Drain any new fw-apply requests, run the typed-armed verb, publish
        /// the result, and on a successful apply re-run SURFACE-4's verify.
        fn poll_once(&mut self, persist: &Persist) {
            self.publish_inventory(persist);
            let topic = fw_apply_topic(&self.node_id);
            let Ok(msgs) = persist.list_since(&topic, self.action_cursor.as_deref()) else {
                return;
            };
            for msg in msgs {
                self.action_cursor = Some(msg.ulid.clone());
                let result = self.apply_request(msg.body.as_deref());
                self.publish_result(persist, &result);
                // Verify re-runs after a successful firmware change (lock #8).
                if result.reverify {
                    self.reverify(persist);
                }
            }
        }

        /// Authenticate and decode one raw Bus request, then hand the typed
        /// human-arm token to the firmware verb. Parsing is deliberately
        /// side-effect free; the shared exact-body gate runs before
        /// [`run_apply`] or any fwupd/backend seam is reached.
        fn apply_request(&self, body: Option<&str>) -> ApplyResult {
            let Some(body) = body else {
                return self.refused_result("", "firmware apply request body is missing");
            };
            let req =
                match FwApplyRequest::from_json_at(body.as_bytes(), &self.node_id, wall_now_ms()) {
                    Ok(req) => req,
                    Err(error) => {
                        return self.refused_result(
                            "",
                            &format!(
                                "firmware apply request failed shared contract admission: {error}"
                            ),
                        );
                    }
                };
            let device_id = req.device_id.trim();
            if device_id.is_empty() {
                return self
                    .refused_result(device_id, "firmware apply request is missing device_id");
            }
            let context = MutationContext {
                verb: FW_ACTION_AUTH_VERB,
                node: &self.node_id,
                target: device_id,
            };
            if let Err(error) = self.authorizer.authorize(body, context) {
                tracing::warn!(
                    target: "mackesd::surface_firmware",
                    node = %self.node_id,
                    device = %device_id,
                    %error,
                    "refused unauthorized firmware apply"
                );
                return self.refused_result(
                    device_id,
                    &format!("firmware apply authorization refused: {error}"),
                );
            }
            run_apply(
                &LiveFwupd,
                &self.detection,
                device_id,
                req.arm_token.as_deref(),
                req.inventory_published_at_ms,
                wall_now_ms(),
                &req.release_version,
                &req.release_checksum,
            )
        }

        fn refused_result(&self, device_id: &str, reason: &str) -> ApplyResult {
            let model = match &self.detection.model {
                SurfaceModel::Known(device) => device.product.clone(),
                SurfaceModel::UnknownSurface { product } => product.clone(),
                SurfaceModel::NotASurface => String::new(),
            };
            ApplyResult {
                model,
                skipped: None,
                device_id: device_id.to_string(),
                outcome: super::ApplyOutcome::Refused {
                    reason: reason.to_string(),
                },
                reverify: false,
            }
        }

        /// Publish the typed apply result to the per-node result lane.
        fn publish_result(&self, persist: &Persist, result: &ApplyResult) {
            publish(persist, &fw_result_topic(&self.node_id), result);
        }

        /// Re-run SURFACE-4's verify and re-publish the board + compact summary
        /// (reusing verify's own hook), so the Test tab + fleet rollup reflect
        /// the freshly-applied firmware.
        fn reverify(&self, persist: &Persist) {
            let private = run_verify(&LiveSurfaceProbes, &self.detection);
            match shared_board(&self.node_id, &self.detection, &private, wall_now_ms()) {
                Ok(board) => {
                    publish(persist, &board_topic(&self.node_id), &board);
                    publish(
                        persist,
                        &summary_topic(&self.node_id),
                        &shared_summary(&board),
                    );
                }
                Err(error) => tracing::warn!(
                    target: "mackesd::surface_firmware",
                    node = %self.node_id,
                    %error,
                    "refusing invalid post-firmware Surface verification publication"
                ),
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

    /// Publish a serializable payload to `topic` (best-effort; a failed write
    /// is logged, not fatal).
    fn publish<T: serde::Serialize>(persist: &Persist, topic: &str, payload: &T) {
        let Ok(body) = serde_json::to_string(payload) else {
            return;
        };
        if let Err(e) = persist.write(topic, Priority::Default, None, Some(&body)) {
            tracing::debug!(
                target: "mackesd::surface_firmware",
                topic,
                error = %e,
                "firmware publish failed"
            );
        }
    }

    /// The default Bus root (same shape the other bus workers use).
    fn default_bus_root() -> Option<PathBuf> {
        mde_bus::default_data_dir()
    }

    #[async_trait::async_trait]
    impl Worker for SurfaceFirmwareWorker {
        fn name(&self) -> &'static str {
            "surface_firmware"
        }

        async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
            // Non-Surface node: the card never appears, so the worker idles
            // (it never touches the Bus) rather than spin.
            if !self.detection.model.is_surface() {
                tracing::debug!(
                    target: "mackesd::surface_firmware",
                    "not a Surface; worker idle"
                );
                shutdown.wait().await;
                return Ok(());
            }
            let Some(root) = self.bus_root.clone() else {
                tracing::debug!(target: "mackesd::surface_firmware", "no bus root; worker idle");
                shutdown.wait().await;
                return Ok(());
            };
            loop {
                match Persist::open(root.clone()) {
                    Ok(persist) => self.poll_once(&persist),
                    Err(e) => tracing::debug!(
                        target: "mackesd::surface_firmware",
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
        use crate::surface::firmware::ApplyOutcome;
        use crate::surface::{DmiInfo, MS_VENDOR, identify};

        const AUTH_KEY: &[u8] = b"surface-firmware-action-auth-test-key";
        const AUTH_NOW: i64 = 1_700_000_000_000;

        fn detection(product: &str) -> SurfaceDetection {
            let dmi = DmiInfo {
                sys_vendor: MS_VENDOR.to_string(),
                product_name: product.to_string(),
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
                let got = SurfaceFirmwareWorker::new("node-a".into()).bus_root;
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
        ) -> SurfaceFirmwareWorker {
            let authorizer = Arc::new(ActionAuthorizer::for_test(
                AUTH_KEY,
                root.join("auth"),
                AUTH_NOW,
            ));
            SurfaceFirmwareWorker::with_parts_and_authorizer(
                node.to_string(),
                detection,
                root.to_path_buf(),
                authorizer,
            )
        }

        fn signed_request(
            node: &str,
            device_id: &str,
            arm_token: Option<&str>,
            nonce: &str,
        ) -> String {
            let unsigned = serde_json::to_string(&FwApplyRequest {
                header: mackes_mesh_types::surface_hardware::SurfaceActionHeader {
                    schema_version:
                        mackes_mesh_types::surface_hardware::SURFACE_HARDWARE_SCHEMA_VERSION,
                    node: node.to_string(),
                    request_id: nonce.to_string(),
                    issued_at_ms: wall_now_ms(),
                    armed_token: None,
                },
                device_id: device_id.to_string(),
                inventory_published_at_ms: wall_now_ms(),
                release_version: "1.2.4".into(),
                release_checksum: "a".repeat(64),
                arm_token: arm_token.map(str::to_string),
            })
            .expect("serialize shared firmware request");
            authorize_test_body(
                AUTH_KEY,
                &unsigned,
                MutationContext {
                    verb: FW_ACTION_AUTH_VERB,
                    node,
                    target: device_id,
                },
                nonce,
                AUTH_NOW + 30_000,
            )
        }

        #[test]
        fn publishes_the_inventory_for_a_surface() {
            let dir = tempfile::tempdir().expect("tempdir");
            let persist = Persist::open(dir.path().to_path_buf()).expect("open bus");
            let w = SurfaceFirmwareWorker::with_parts(
                "node-a".into(),
                detection("Surface Pro 8"),
                dir.path().to_path_buf(),
            );

            w.publish_inventory(&persist);

            let items = persist
                .list_since(&inventory_topic("node-a"), None)
                .expect("list inventory");
            assert_eq!(items.len(), 1, "one inventory published");
            let inv = mackes_mesh_types::surface_hardware::SurfaceFirmwareInventory::from_json(
                items[0].body.as_deref().unwrap().as_bytes(),
            )
            .expect("shared firmware inventory");
            assert_eq!(inv.publication.model.product, "Surface Pro 8");
            assert!(inv.skipped.is_none() || inv.devices.is_empty());
        }

        #[test]
        fn drains_an_unarmed_apply_and_refuses_no_reverify() {
            let dir = tempfile::tempdir().expect("tempdir");
            let persist = Persist::open(dir.path().to_path_buf()).expect("open bus");
            let mut w = SurfaceFirmwareWorker::with_parts(
                "node-a".into(),
                detection("Surface Pro 8"),
                dir.path().to_path_buf(),
            );

            // The Install tab requests an apply WITHOUT the arm token.
            let req = serde_json::to_string(&FwApplyRequest {
                header: mackes_mesh_types::surface_hardware::SurfaceActionHeader {
                    schema_version:
                        mackes_mesh_types::surface_hardware::SURFACE_HARDWARE_SCHEMA_VERSION,
                    node: "node-a".into(),
                    request_id: "unarmed-apply".into(),
                    issued_at_ms: wall_now_ms(),
                    armed_token: None,
                },
                device_id: "uefi-1".into(),
                inventory_published_at_ms: wall_now_ms(),
                release_version: "1.2.4".into(),
                release_checksum: "a".repeat(64),
                arm_token: None,
            })
            .unwrap();
            persist
                .write(
                    &fw_apply_topic("node-a"),
                    Priority::Default,
                    None,
                    Some(&req),
                )
                .expect("write request");

            w.poll_once(&persist);

            let out = persist
                .list_since(&fw_result_topic("node-a"), None)
                .expect("list results");
            assert_eq!(out.len(), 1, "one apply result published");
            let result: ApplyResult =
                serde_json::from_str(out[0].body.as_deref().unwrap()).unwrap();
            assert!(matches!(
                result.outcome,
                super::super::ApplyOutcome::Refused { .. }
            ));
            assert!(!result.reverify, "a refused apply does not re-verify");

            // No verify board was re-published (nothing changed).
            let boards = persist
                .list_since(&board_topic("node-a"), None)
                .expect("list boards");
            assert!(boards.is_empty(), "no re-verify on a refused apply");
        }

        #[test]
        fn typed_arm_without_hmac_capability_is_refused_before_fwupd() {
            let dir = tempfile::tempdir().expect("tempdir");
            let persist = Persist::open(dir.path().to_path_buf()).expect("open bus");
            let mut w = SurfaceFirmwareWorker::with_parts(
                "node-a".into(),
                detection("Surface Pro 8"),
                dir.path().to_path_buf(),
            );
            let request = serde_json::json!({
                "schema_version": 1,
                "device_id": "uefi-1",
                "arm_token": super::super::FW_ARM_TOKEN,
            })
            .to_string();
            persist
                .write(
                    &fw_apply_topic("node-a"),
                    Priority::Default,
                    None,
                    Some(&request),
                )
                .expect("write request");

            w.poll_once(&persist);

            let out = persist
                .list_since(&fw_result_topic("node-a"), None)
                .expect("list results");
            let result: ApplyResult =
                serde_json::from_str(out[0].body.as_deref().unwrap()).unwrap();
            let ApplyOutcome::Refused { reason } = result.outcome else {
                panic!("missing authorization refusal");
            };
            assert!(reason.contains("shared contract admission"));
            assert!(!result.reverify);
        }

        #[test]
        fn duplicate_and_foreign_firmware_fields_fail_shared_admission() {
            let dir = tempfile::tempdir().expect("tempdir");
            let worker = authorized_worker("node-a", detection("Surface Pro 6"), dir.path());
            let duplicate = format!(
                r#"{{"schema_version":1,"node":"node-a","request_id":"fw-duplicate","issued_at_ms":{},"device_id":"uefi-1","device_id":"uefi-2"}}"#,
                wall_now_ms()
            );
            let result = worker.apply_request(Some(&duplicate));
            let ApplyOutcome::Refused { reason } = result.outcome else {
                panic!("duplicate key reached the effect path");
            };
            assert!(reason.contains("shared contract admission"));

            let foreign = FwApplyRequest {
                header: mackes_mesh_types::surface_hardware::SurfaceActionHeader {
                    schema_version:
                        mackes_mesh_types::surface_hardware::SURFACE_HARDWARE_SCHEMA_VERSION,
                    node: "node-b".into(),
                    request_id: "fw-foreign".into(),
                    issued_at_ms: wall_now_ms(),
                    armed_token: None,
                },
                device_id: "uefi-1".into(),
                inventory_published_at_ms: wall_now_ms(),
                release_version: "1.2.4".into(),
                release_checksum: "a".repeat(64),
                arm_token: None,
            };
            let result = worker.apply_request(Some(
                &serde_json::to_string(&foreign).expect("serialize foreign request"),
            ));
            let ApplyOutcome::Refused { reason } = result.outcome else {
                panic!("foreign request reached the effect path");
            };
            assert!(reason.contains("different node"));
        }

        #[test]
        fn valid_hmac_then_typed_arm_revalidates_live_inventory() {
            let dir = tempfile::tempdir().expect("tempdir");
            let persist = Persist::open(dir.path().to_path_buf()).expect("open bus");
            let mut w = authorized_worker("node-auth", detection("Surface Pro 6"), dir.path());
            let request = signed_request(
                "node-auth",
                "uefi-1",
                Some(super::super::FW_ARM_TOKEN),
                "surface-fw-valid",
            );
            persist
                .write(
                    &fw_apply_topic("node-auth"),
                    Priority::Default,
                    None,
                    Some(&request),
                )
                .expect("write request");

            w.poll_once(&persist);

            let out = persist
                .list_since(&fw_result_topic("node-auth"), None)
                .expect("list results");
            let result: ApplyResult =
                serde_json::from_str(out[0].body.as_deref().unwrap()).unwrap();
            let ApplyOutcome::Refused { reason } = result.outcome else {
                panic!("unbound firmware request was not refused");
            };
            assert!(reason.contains("fwupdmgr"));
            assert!(!result.reverify);
        }

        #[test]
        fn hmac_success_does_not_replace_the_typed_arm_interlock() {
            let dir = tempfile::tempdir().expect("tempdir");
            let persist = Persist::open(dir.path().to_path_buf()).expect("open bus");
            let mut w = authorized_worker("node-arm", detection("Surface Pro 6"), dir.path());
            let request = signed_request("node-arm", "uefi-1", None, "surface-fw-unarmed");
            persist
                .write(
                    &fw_apply_topic("node-arm"),
                    Priority::Default,
                    None,
                    Some(&request),
                )
                .expect("write request");

            w.poll_once(&persist);

            let out = persist
                .list_since(&fw_result_topic("node-arm"), None)
                .expect("list results");
            let result: ApplyResult =
                serde_json::from_str(out[0].body.as_deref().unwrap()).unwrap();
            let ApplyOutcome::Refused { reason } = result.outcome else {
                panic!("missing typed-arm refusal");
            };
            assert!(reason.contains("not armed"));
        }

        #[test]
        fn body_tampering_and_capability_replay_are_refused() {
            let dir = tempfile::tempdir().expect("tempdir");
            let persist = Persist::open(dir.path().to_path_buf()).expect("open bus");
            let mut w = authorized_worker("node-replay", detection("Surface Pro 8"), dir.path());
            let original = signed_request(
                "node-replay",
                "uefi-1",
                Some(super::super::FW_ARM_TOKEN),
                "surface-fw-replay",
            );
            let tampered = original.replace("uefi-1", "uefi-2");
            for request in [&tampered, &original, &original] {
                persist
                    .write(
                        &fw_apply_topic("node-replay"),
                        Priority::Default,
                        None,
                        Some(request),
                    )
                    .expect("write request");
            }

            w.poll_once(&persist);

            let out = persist
                .list_since(&fw_result_topic("node-replay"), None)
                .expect("list results");
            assert_eq!(out.len(), 3);
            let results: Vec<ApplyResult> = out
                .iter()
                .map(|item| serde_json::from_str(item.body.as_deref().unwrap()).unwrap())
                .collect();
            assert!(matches!(results[0].outcome, ApplyOutcome::Refused { .. }));
            assert!(matches!(results[1].outcome, ApplyOutcome::Refused { .. }));
            let ApplyOutcome::Refused { reason } = &results[2].outcome else {
                panic!("replay was not refused");
            };
            assert!(reason.contains("already used"));
        }

        #[test]
        fn cursor_advances_so_a_request_is_processed_once() {
            let dir = tempfile::tempdir().expect("tempdir");
            let persist = Persist::open(dir.path().to_path_buf()).expect("open bus");
            let mut w = SurfaceFirmwareWorker::with_parts(
                "n".into(),
                detection("Surface Pro 8"),
                dir.path().to_path_buf(),
            );
            let req = serde_json::to_string(&FwApplyRequest {
                header: mackes_mesh_types::surface_hardware::SurfaceActionHeader {
                    schema_version:
                        mackes_mesh_types::surface_hardware::SURFACE_HARDWARE_SCHEMA_VERSION,
                    node: "n".into(),
                    request_id: "cursor-apply".into(),
                    issued_at_ms: wall_now_ms(),
                    armed_token: None,
                },
                device_id: "uefi-1".into(),
                inventory_published_at_ms: wall_now_ms(),
                release_version: "1.2.4".into(),
                release_checksum: "a".repeat(64),
                arm_token: None,
            })
            .unwrap();
            persist
                .write(&fw_apply_topic("n"), Priority::Default, None, Some(&req))
                .expect("write");
            w.poll_once(&persist);
            w.poll_once(&persist); // second drain: no new request
            let out = persist
                .list_since(&fw_result_topic("n"), None)
                .expect("list");
            assert_eq!(out.len(), 1, "request processed exactly once");
        }

        // A verify board round-trips through the reverify hook's topic, proving
        // the re-verify path publishes a real SURFACE-4 board.
        #[test]
        fn reverify_publishes_a_verify_board() {
            let dir = tempfile::tempdir().expect("tempdir");
            let persist = Persist::open(dir.path().to_path_buf()).expect("open bus");
            let w = SurfaceFirmwareWorker::with_parts(
                "node-r".into(),
                detection("Surface Pro 8"),
                dir.path().to_path_buf(),
            );
            w.reverify(&persist);
            let boards = persist
                .list_since(&board_topic("node-r"), None)
                .expect("list boards");
            assert_eq!(boards.len(), 1, "a verify board was re-published");
            let board = mackes_mesh_types::surface_hardware::SurfaceVerifyBoard::from_json(
                boards[0].body.as_deref().unwrap().as_bytes(),
            )
            .expect("shared verify board");
            assert_eq!(board.publication.model.product, "Surface Pro 8");
        }
    }
}

// ─────────────────────────────── tests ──────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::{DmiInfo, MS_VENDOR, identify};

    const APPLY_NOW_MS: u64 = 1_800_000_000_000;
    const RELEASE_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    /// A scripted fake fwupd seam so the parse + apply run without a machine.
    #[derive(Clone)]
    struct FakeFwupd {
        devices_json: Result<String, FwError>,
        updates_json: Result<String, FwError>,
        apply: Result<(), FwError>,
    }

    impl Default for FakeFwupd {
        /// A benign default: empty device lists + a successful apply, so a test
        /// only scripts the field it cares about.
        fn default() -> Self {
            Self {
                devices_json: Ok(EMPTY_DEVICE_LIST.to_string()),
                updates_json: Ok(EMPTY_DEVICE_LIST.to_string()),
                apply: Ok(()),
            }
        }
    }

    impl Fwupd for FakeFwupd {
        fn get_devices_json(&self) -> Result<String, FwError> {
            self.devices_json.clone()
        }
        fn get_updates_json(&self) -> Result<String, FwError> {
            self.updates_json.clone()
        }
        fn apply_update(
            &self,
            _device_id: &str,
            _release_version: &str,
            _release_checksum: &str,
        ) -> Result<(), FwError> {
            self.apply.clone()
        }
    }

    fn detect_of(product: &str) -> SurfaceDetection {
        let dmi = DmiInfo {
            sys_vendor: MS_VENDOR.to_string(),
            product_name: product.to_string(),
            ..Default::default()
        };
        SurfaceDetection {
            model: identify(&dmi),
            dmi,
        }
    }

    // Real-shape fwupd `get-devices --json` fixture: a System Firmware device
    // and a UEFI dbx device.
    const DEVICES_JSON: &str = r#"{
      "Devices": [
        { "DeviceId": "sysfw-1", "Name": "System Firmware", "Version": "1.2.3", "Plugin": "uefi_capsule" },
        { "DeviceId": "dbx-1", "Name": "UEFI dbx", "Version": "20230101", "Plugin": "uefi_dbx" },
        { "DeviceId": "touch-1", "Name": "Touch Controller", "Version": "5.0.0", "Plugin": "surface_touch" }
      ]
    }"#;

    // `get-updates --json`: System Firmware has a newer release; the dbx device
    // lists a release that is NOT newer (already current); touch has none.
    const UPDATES_JSON: &str = r#"{
      "Devices": [
        { "DeviceId": "sysfw-1", "Name": "System Firmware", "Version": "1.2.3", "Releases": [ { "Version": "1.2.4", "Checksum": ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"] } ] },
        { "DeviceId": "dbx-1", "Name": "UEFI dbx", "Version": "20230101", "Releases": [ { "Version": "20230101", "Checksum": ["bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"] } ] }
      ]
    }"#;

    // ── the version compare fold ────────────────────────────────────────────

    #[test]
    fn version_newer_compares_numeric_components() {
        assert!(version_newer("1.2.4", "1.2.3"));
        assert!(version_newer("1.2.10", "1.2.9"));
        assert!(version_newer("20240601", "20230101"));
        assert!(!version_newer("1.2.3", "1.2.3"));
        assert!(!version_newer("1.2.3", "1.2.4"));
        // A missing trailing component reads as 0.
        assert!(version_newer("1.3", "1.2.9"));
        assert!(!version_newer("1.2", "1.2.0"));
    }

    // ── the JSON parse + update-available fold ──────────────────────────────

    #[test]
    fn inventory_parses_devices_and_matches_available_versions() {
        let devices = inventory_from_json(DEVICES_JSON, UPDATES_JSON).expect("parse");
        assert_eq!(devices.len(), 3);

        let sysfw = devices.iter().find(|d| d.device_id == "sysfw-1").unwrap();
        assert_eq!(sysfw.name, "System Firmware");
        assert_eq!(sysfw.current_version, "1.2.3");
        assert_eq!(sysfw.available_version.as_deref(), Some("1.2.4"));
        assert_eq!(sysfw.available_checksum.as_deref(), Some(RELEASE_SHA256));
        assert!(sysfw.update_available, "1.2.4 > 1.2.3 is a real update");

        // A release that isn't newer is honestly NOT an update.
        let dbx = devices.iter().find(|d| d.device_id == "dbx-1").unwrap();
        assert_eq!(dbx.available_version.as_deref(), Some("20230101"));
        assert!(!dbx.update_available, "same version is not an update");

        // No release at all → no available version, no update.
        let touch = devices.iter().find(|d| d.device_id == "touch-1").unwrap();
        assert_eq!(touch.available_version, None);
        assert!(!touch.update_available);
    }

    #[test]
    fn inventory_with_no_updates_lists_everything_up_to_date() {
        let devices = inventory_from_json(DEVICES_JSON, r#"{"Devices":[]}"#).expect("parse");
        assert_eq!(devices.len(), 3);
        assert!(devices.iter().all(|d| !d.update_available));
        assert!(devices.iter().all(|d| d.available_version.is_none()));
    }

    #[test]
    fn malformed_json_is_an_honest_error_not_a_panic() {
        let err = inventory_from_json("not json", "{}").unwrap_err();
        assert!(matches!(err, FwError::Failed { .. }));
    }

    #[test]
    fn inventory_rejects_duplicate_oversized_and_excess_rows() {
        let duplicate = r#"{"Devices":[{"DeviceId":"same","Name":"A","Version":"1"},{"DeviceId":"same","Name":"B","Version":"2"}]}"#;
        assert!(inventory_from_json(duplicate, EMPTY_DEVICE_LIST).is_err());

        let oversized = serde_json::json!({"Devices": [{
            "DeviceId": "x",
            "Name": "x".repeat(MAX_FWUPD_FIELD_BYTES + 1),
            "Version": "1"
        }]});
        assert!(inventory_from_json(&oversized.to_string(), EMPTY_DEVICE_LIST).is_err());

        let rows: Vec<_> = (0..=MAX_FWUPD_DEVICES)
            .map(|index| {
                serde_json::json!({
                    "DeviceId": format!("device-{index}"), "Name": "Device", "Version": "1"
                })
            })
            .collect();
        assert!(
            inventory_from_json(
                &serde_json::json!({"Devices": rows}).to_string(),
                EMPTY_DEVICE_LIST
            )
            .is_err()
        );
    }

    #[test]
    fn command_capture_discards_beyond_the_hard_limit() {
        let captured = read_bounded(std::io::Cursor::new(vec![b'x'; MAX_FWUPD_OUTPUT_BYTES + 1]))
            .expect("bounded read");
        assert_eq!(captured.bytes.len(), MAX_FWUPD_OUTPUT_BYTES);
        assert!(captured.truncated);
    }

    #[test]
    fn live_read_argv_is_fixed_json_only() {
        assert_eq!(FWUPDMGR, "/usr/bin/fwupdmgr");
        assert_eq!(
            GET_DEVICES_ARGS,
            ["get-devices", "--json", "--no-unreported-check"]
        );
        assert_eq!(
            GET_UPDATES_ARGS,
            ["get-updates", "--json", "--no-unreported-check"]
        );
        assert!(!GET_DEVICES_ARGS.iter().any(|arg| arg.contains("update")));
        assert!(!GET_UPDATES_ARGS.iter().any(|arg| *arg == "update"));
    }

    #[test]
    fn inventory_rejects_duplicate_json_keys() {
        assert!(inventory_from_json(r#"{"Devices":[],"Devices":[]}"#, EMPTY_DEVICE_LIST).is_err());
    }

    // ── the inventory verb ──────────────────────────────────────────────────

    #[test]
    fn run_inventory_folds_the_seam_for_a_surface() {
        let fake = FakeFwupd {
            devices_json: Ok(DEVICES_JSON.to_string()),
            updates_json: Ok(UPDATES_JSON.to_string()),
            apply: Ok(()),
        };
        let inv = run_inventory(&fake, &detect_of("Surface Pro 8"));
        assert_eq!(inv.model, "Surface Pro 8");
        assert!(inv.skipped.is_none());
        assert_eq!(inv.devices.len(), 3);
        assert_eq!(
            inv.update_count(),
            1,
            "only System Firmware has a real update"
        );
    }

    #[test]
    fn run_inventory_skips_a_non_surface() {
        let dmi = DmiInfo {
            sys_vendor: "Dell Inc.".into(),
            product_name: "XPS 13".into(),
            ..Default::default()
        };
        let det = SurfaceDetection {
            model: identify(&dmi),
            dmi,
        };
        let inv = run_inventory(&FakeFwupd::default(), &det);
        assert_eq!(inv.skipped.as_deref(), Some("not a Microsoft Surface"));
        assert!(inv.devices.is_empty());
    }

    #[test]
    fn run_inventory_gated_read_is_honest_skip_never_faked() {
        let fake = FakeFwupd {
            devices_json: Err(FwError::IntegrationGated {
                action: "fwupdmgr get-devices".into(),
            }),
            ..Default::default()
        };
        let inv = run_inventory(&fake, &detect_of("Surface Pro 8"));
        assert!(
            inv.skipped
                .as_deref()
                .unwrap()
                .contains("integration-gated")
        );
        assert!(inv.devices.is_empty(), "never a fabricated device list");
    }

    #[test]
    fn failed_updates_query_never_fabricates_up_to_date_rows() {
        let fake = FakeFwupd {
            devices_json: Ok(DEVICES_JSON.to_string()),
            updates_json: Err(FwError::Failed {
                action: "fwupdmgr get-updates".into(),
                detail: "daemon unavailable".into(),
            }),
            apply: Ok(()),
        };
        let inventory = run_inventory(&fake, &detect_of("Surface Pro 6"));
        assert!(
            inventory
                .skipped
                .as_deref()
                .is_some_and(|reason| reason.contains("daemon unavailable"))
        );
        assert!(inventory.devices.is_empty());
    }

    // ── the typed-armed apply verb ──────────────────────────────────────────

    #[test]
    fn unarmed_apply_is_refused_and_runs_nothing() {
        let fake = FakeFwupd {
            apply: Ok(()),
            ..Default::default()
        };
        let r = run_apply(
            &fake,
            &detect_of("Surface Pro 6"),
            "sysfw-1",
            None,
            APPLY_NOW_MS,
            APPLY_NOW_MS,
            "1.2.4",
            RELEASE_SHA256,
        );
        assert!(matches!(r.outcome, ApplyOutcome::Refused { .. }));
        assert!(!r.reverify, "a refused apply does not re-verify");
    }

    #[test]
    fn wrong_arm_token_is_refused() {
        let fake = FakeFwupd {
            apply: Ok(()),
            ..Default::default()
        };
        let r = run_apply(
            &fake,
            &detect_of("Surface Pro 6"),
            "sysfw-1",
            Some("nope"),
            APPLY_NOW_MS,
            APPLY_NOW_MS,
            "1.2.4",
            RELEASE_SHA256,
        );
        assert!(matches!(r.outcome, ApplyOutcome::Refused { .. }));
    }

    #[test]
    fn armed_apply_without_generation_release_and_checksum_is_refused() {
        let fake = FakeFwupd {
            apply: Ok(()),
            ..Default::default()
        };
        let r = run_apply(
            &fake,
            &detect_of("Surface Pro 6"),
            "sysfw-1",
            Some(FW_ARM_TOKEN),
            0,
            APPLY_NOW_MS,
            "",
            "",
        );
        let ApplyOutcome::Refused { reason } = r.outcome else {
            panic!("unbound apply was not refused");
        };
        assert!(reason.contains("stale") || reason.contains("invalid release binding"));
        assert_eq!(r.device_id, "sysfw-1");
        assert!(!r.reverify);
    }

    #[test]
    fn missing_bindings_prevent_the_apply_seam_from_being_called() {
        struct PanicApply;
        impl Fwupd for PanicApply {
            fn get_devices_json(&self) -> Result<String, FwError> {
                Ok(EMPTY_DEVICE_LIST.into())
            }
            fn get_updates_json(&self) -> Result<String, FwError> {
                Ok(EMPTY_DEVICE_LIST.into())
            }
            fn apply_update(
                &self,
                _device_id: &str,
                _release_version: &str,
                _release_checksum: &str,
            ) -> Result<(), FwError> {
                panic!("unsafe fwupd apply seam reached")
            }
        }
        let result = run_apply(
            &PanicApply,
            &detect_of("Surface Pro 6"),
            "sysfw-1",
            Some(FW_ARM_TOKEN),
            0,
            APPLY_NOW_MS,
            "",
            "",
        );
        assert!(matches!(result.outcome, ApplyOutcome::Refused { .. }));
        assert!(!result.reverify);
    }

    #[test]
    fn armed_apply_failure_is_honest_and_does_not_reverify() {
        let fake = FakeFwupd {
            devices_json: Ok(DEVICES_JSON.to_string()),
            updates_json: Ok(UPDATES_JSON.to_string()),
            apply: Err(FwError::Failed {
                action: "fwupdmgr update sysfw-1".into(),
                detail: "device rejected the update".into(),
            }),
        };
        let r = run_apply(
            &fake,
            &detect_of("Surface Pro 6"),
            "sysfw-1",
            Some(FW_ARM_TOKEN),
            APPLY_NOW_MS,
            APPLY_NOW_MS,
            "1.2.4",
            RELEASE_SHA256,
        );
        assert!(matches!(r.outcome, ApplyOutcome::Failed { .. }));
        assert!(!r.reverify);
    }

    #[test]
    fn exact_fresh_release_binding_reaches_the_apply_seam() {
        let fake = FakeFwupd {
            devices_json: Ok(DEVICES_JSON.to_string()),
            updates_json: Ok(UPDATES_JSON.to_string()),
            apply: Ok(()),
        };
        let result = run_apply(
            &fake,
            &detect_of("Surface Pro 6"),
            "sysfw-1",
            Some(FW_ARM_TOKEN),
            APPLY_NOW_MS,
            APPLY_NOW_MS,
            "1.2.4",
            RELEASE_SHA256,
        );
        assert_eq!(result.outcome, ApplyOutcome::Applied);
        assert!(result.reverify);
    }

    #[test]
    fn armed_apply_live_is_refused_before_fwupdmgr() {
        let r = run_apply(
            &LiveFwupd,
            &detect_of("Surface Pro 6"),
            "sysfw-1",
            Some(FW_ARM_TOKEN),
            APPLY_NOW_MS,
            APPLY_NOW_MS,
            "1.2.4",
            RELEASE_SHA256,
        );
        assert!(matches!(r.outcome, ApplyOutcome::Refused { .. }));
        assert!(!r.reverify);
    }

    #[test]
    fn apply_skips_a_non_surface() {
        let dmi = DmiInfo {
            sys_vendor: "Dell Inc.".into(),
            product_name: "XPS 13".into(),
            ..Default::default()
        };
        let det = SurfaceDetection {
            model: identify(&dmi),
            dmi,
        };
        let r = run_apply(
            &FakeFwupd::default(),
            &det,
            "sysfw-1",
            Some(FW_ARM_TOKEN),
            APPLY_NOW_MS,
            APPLY_NOW_MS,
            "1.2.4",
            RELEASE_SHA256,
        );
        assert_eq!(r.skipped.as_deref(), Some("not a Microsoft Surface"));
    }

    #[test]
    fn triggers_reverify_only_on_applied() {
        assert!(ApplyOutcome::Applied.triggers_reverify());
        assert!(
            !ApplyOutcome::Refused {
                reason: String::new()
            }
            .triggers_reverify()
        );
        assert!(
            !ApplyOutcome::Gated {
                reason: String::new()
            }
            .triggers_reverify()
        );
        assert!(
            !ApplyOutcome::Failed {
                reason: String::new()
            }
            .triggers_reverify()
        );
    }

    #[test]
    fn live_inventory_is_either_real_or_explicitly_unavailable() {
        let inv = run_inventory(&LiveFwupd, &detect_of("Surface Pro 8"));
        assert!(inv.skipped.is_none() || inv.devices.is_empty());
    }
}
