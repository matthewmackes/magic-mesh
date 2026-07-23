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
//! the production seam ([`LiveFwupd`]) is integration-gated — an honest typed
//! [`FwError::IntegrationGated`] when fwupd/network is absent (§7 — it never
//! fakes an update, and §9 — the typed verb layer, no raw shell string
//! escapes this module). §6-clean: it stays wholly in mackesd.

use serde::{Deserialize, Serialize};

use super::{SurfaceDetection, SurfaceModel};

/// The exact token the operator must type to arm a firmware apply (lock #8 —
/// never an auto-apply).
///
/// Deliberately unambiguous; the Install tab shows it and the apply request
/// echoes it back in `arm_token`. Mirrors SURFACE-3's
/// [`super::enable::MOK_ARM_TOKEN`] interlock.
pub const FW_ARM_TOKEN: &str = "APPLY-SURFACE-FIRMWARE";

// ─────────────────────────────── the seam ───────────────────────────────────

/// A typed failure from the [`Fwupd`] seam — mirrors
/// [`super::enable::EnableError`]'s honest split.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FwError {
    /// The live fwupd call isn't wired to real hardware/network yet — the
    /// honest answer on any non-Surface dev box / headless CI (§7: never a
    /// faked update). `action` names what was gated.
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
/// Each method returns [`FwError::IntegrationGated`] when the live call is
/// integration-gated (no fwupd / no Surface) and [`FwError::Failed`] on a
/// concrete failure.
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
    /// Apply the pending update for `device_id` (`fwupdmgr update <id>`). Only
    /// ever called after the typed arm matched.
    ///
    /// # Errors
    /// The seam's typed [`FwError`] (gated / failed) — see the trait docs.
    fn apply_update(&self, device_id: &str) -> Result<(), FwError>;
}

/// The production seam. **Integration-gated** (lock #8).
///
/// Every live fwupd call returns an honest [`FwError::IntegrationGated`] until
/// it's wired to a real fwupd/LVFS at integration — never a faked update. The
/// pure parse + apply fold is fully exercised behind [`Fwupd`] with a fake.
#[derive(Debug, Clone, Copy, Default)]
pub struct LiveFwupd;

impl LiveFwupd {
    fn gated<T>(action: impl Into<String>) -> Result<T, FwError> {
        Err(FwError::IntegrationGated {
            action: action.into(),
        })
    }
}

impl Fwupd for LiveFwupd {
    fn get_devices_json(&self) -> Result<String, FwError> {
        Self::gated("fwupdmgr get-devices")
    }
    fn get_updates_json(&self) -> Result<String, FwError> {
        Self::gated("fwupdmgr get-updates")
    }
    fn apply_update(&self, device_id: &str) -> Result<(), FwError> {
        Self::gated(format!("fwupdmgr update {device_id}"))
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
    Ok(merge_inventory(&devices.devices, &updates.devices))
}

/// Merge the raw device list with the raw update list (pure). Kept separate
/// from [`inventory_from_json`] so the merge logic is testable without JSON.
fn merge_inventory(devices: &[RawDevice], updates: &[RawDevice]) -> Vec<FwDevice> {
    devices
        .iter()
        .map(|dev| {
            // The available release is the update list's first `Releases`
            // entry for this device id (fwupd lists the newest first).
            let available_version = updates
                .iter()
                .find(|u| u.device_id == dev.device_id)
                .and_then(|u| u.releases.first())
                .map(|r| r.version.clone())
                .filter(|v| !v.is_empty());
            let update_available = available_version
                .as_deref()
                .is_some_and(|av| version_newer(av, &dev.version));
            FwDevice {
                device_id: dev.device_id.clone(),
                name: dev.name.clone(),
                plugin: dev.plugin.clone(),
                current_version: dev.version.clone(),
                available_version,
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
    // The updates read is best-effort: if fwupd can't tell us what has an
    // update we still show the inventory (every device just reads "up to date"
    // honestly), never a fabricated available version.
    let updates_json = fwupd
        .get_updates_json()
        .unwrap_or_else(|_| EMPTY_DEVICE_LIST.to_string());

    match inventory_from_json(&devices_json, &updates_json) {
        Ok(devices) => FirmwareInventory {
            model,
            skipped: None,
            devices,
        },
        Err(e) => FirmwareInventory::skipped(model, e.to_string()),
    }
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
/// is **refused** and nothing runs (lock #8 — never an auto-apply). A
/// successful apply sets `reverify` so the worker re-runs SURFACE-4's verify.
/// Pure control flow over the injectable [`Fwupd`] seam; production hands
/// [`LiveFwupd`] (integration-gated).
#[must_use]
pub fn run_apply(
    fwupd: &impl Fwupd,
    detection: &SurfaceDetection,
    device_id: &str,
    arm: Option<&str>,
) -> ApplyResult {
    let model = match &detection.model {
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
        SurfaceModel::Known(dev) => dev.product.clone(),
    };

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

    let outcome = match fwupd.apply_update(device_id) {
        Ok(()) => ApplyOutcome::Applied,
        Err(e @ FwError::IntegrationGated { .. }) => ApplyOutcome::Gated {
            reason: e.to_string(),
        },
        Err(e @ FwError::Failed { .. }) => ApplyOutcome::Failed {
            reason: e.to_string(),
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
    fw_apply_topic, fw_result_topic, inventory_topic, FwApplyRequest, SurfaceFirmwareWorker,
    FW_ACTION_AUTH_VERB,
};

#[cfg(feature = "async-services")]
mod worker {
    //! The per-node `surface_firmware` Bus worker (a *leader-of-self* worker:
    //! it reads + updates only its own firmware, never a remote node). Each
    //! tick it publishes the fwupd inventory to [`inventory_topic`]; it drains
    //! [`fw_apply_topic`] for typed-armed apply requests, runs
    //! [`super::run_apply`] against the integration-gated [`super::LiveFwupd`],
    //! publishes the [`super::ApplyResult`] to [`fw_result_topic`], and on a
    //! successful apply re-runs SURFACE-4's verify (reusing
    //! [`crate::surface::verify::run_verify`]) and re-publishes the board +
    //! summary. On a non-Surface node it idles (never touches the Bus).

    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    use mde_bus::hooks::config::Priority;
    use mde_bus::persist::Persist;
    use serde::{Deserialize, Serialize};

    use super::{run_apply, run_inventory, ApplyResult, LiveFwupd, SurfaceModel};
    use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};
    use crate::surface::verify::{
        board_topic, run_verify, summarize, summary_topic, LiveSurfaceProbes,
    };
    use crate::surface::{detect, SurfaceDetection};
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

    /// The fw-apply request envelope. `device_id` names the fwupd device;
    /// `arm_token` carries the operator-typed [`super::FW_ARM_TOKEN`] that
    /// arms the apply (absent = a refused dry request).
    ///
    /// The raw JSON body must additionally carry `schema_version: 1` and an
    /// `armed_token` minted by the shared [`ActionAuthorizer`] for
    /// [`FW_ACTION_AUTH_VERB`], this node id, and `device_id`. The shared gate
    /// authenticates the exact raw body before this typed payload reaches
    /// [`run_apply`]; `armed_token` is intentionally not copied into this
    /// struct so it cannot be confused with the human typed interlock.
    #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub struct FwApplyRequest {
        /// The fwupd device id to update.
        pub device_id: String,
        /// The typed arming token (present only to actually apply).
        #[serde(default)]
        pub arm_token: Option<String>,
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
            publish(persist, &inventory_topic(&self.node_id), &inventory);
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
            let req = match serde_json::from_str::<FwApplyRequest>(body) {
                Ok(req) => req,
                Err(_) => {
                    return self
                        .refused_result("", "firmware apply request is not a valid JSON object")
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
            let board = run_verify(&LiveSurfaceProbes, &self.detection);
            publish(persist, &board_topic(&self.node_id), &board);
            let summary = summarize(&self.node_id, &board);
            publish(persist, &summary_topic(&self.node_id), &summary);
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
                target: "mackesd::surface_firmware",
                topic,
                error = %e,
                "firmware publish failed"
            );
        }
    }

    /// The default Bus root (same shape the other bus workers use).
    fn default_bus_root() -> Option<PathBuf> {
        Some(dirs::data_dir()?.join("mde").join("bus"))
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
                return Ok(());
            }
            let Some(root) = self.bus_root.clone() else {
                tracing::debug!(target: "mackesd::surface_firmware", "no bus root; worker idle");
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
        use crate::ipc::action_auth::{authorize_test_body, ActionAuthorizer, MutationContext};
        use crate::surface::firmware::{ApplyOutcome, FirmwareInventory};
        use crate::surface::verify::VerifyBoard;
        use crate::surface::{identify, DmiInfo, MS_VENDOR};

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
            let unsigned = serde_json::json!({
                "schema_version": 1,
                "device_id": device_id,
                "arm_token": arm_token,
            })
            .to_string();
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
            let inv: FirmwareInventory =
                serde_json::from_str(items[0].body.as_deref().unwrap()).unwrap();
            assert_eq!(inv.model, "Surface Pro 8");
            // Live fwupd is integration-gated headless → an honest skip reason,
            // never a fabricated device list.
            assert!(inv.skipped.is_some());
            assert!(inv.devices.is_empty());
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
                device_id: "uefi-1".into(),
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
            assert!(reason.contains("authorization refused"));
            assert!(!result.reverify);
        }

        #[test]
        fn valid_hmac_then_typed_arm_reaches_live_fwupd_seam() {
            let dir = tempfile::tempdir().expect("tempdir");
            let persist = Persist::open(dir.path().to_path_buf()).expect("open bus");
            let mut w = authorized_worker("node-auth", detection("Surface Pro 8"), dir.path());
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
            assert!(matches!(result.outcome, ApplyOutcome::Gated { .. }));
            assert!(!result.reverify, "headless LiveFwupd remains gated");
        }

        #[test]
        fn hmac_success_does_not_replace_the_typed_arm_interlock() {
            let dir = tempfile::tempdir().expect("tempdir");
            let persist = Persist::open(dir.path().to_path_buf()).expect("open bus");
            let mut w = authorized_worker("node-arm", detection("Surface Pro 8"), dir.path());
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
            assert!(matches!(results[1].outcome, ApplyOutcome::Gated { .. }));
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
                device_id: "uefi-1".into(),
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
            let board: VerifyBoard =
                serde_json::from_str(boards[0].body.as_deref().unwrap()).unwrap();
            assert_eq!(board.model, "Surface Pro 8");
        }
    }
}

// ─────────────────────────────── tests ──────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::{identify, DmiInfo, MS_VENDOR};

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
        fn apply_update(&self, _device_id: &str) -> Result<(), FwError> {
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
        { "DeviceId": "sysfw-1", "Name": "System Firmware", "Version": "1.2.3", "Releases": [ { "Version": "1.2.4" } ] },
        { "DeviceId": "dbx-1", "Name": "UEFI dbx", "Version": "20230101", "Releases": [ { "Version": "20230101" } ] }
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
        assert!(inv
            .skipped
            .as_deref()
            .unwrap()
            .contains("integration-gated"));
        assert!(inv.devices.is_empty(), "never a fabricated device list");
    }

    // ── the typed-armed apply verb ──────────────────────────────────────────

    #[test]
    fn unarmed_apply_is_refused_and_runs_nothing() {
        let fake = FakeFwupd {
            apply: Ok(()),
            ..Default::default()
        };
        let r = run_apply(&fake, &detect_of("Surface Pro 8"), "sysfw-1", None);
        assert!(matches!(r.outcome, ApplyOutcome::Refused { .. }));
        assert!(!r.reverify, "a refused apply does not re-verify");
    }

    #[test]
    fn wrong_arm_token_is_refused() {
        let fake = FakeFwupd {
            apply: Ok(()),
            ..Default::default()
        };
        let r = run_apply(&fake, &detect_of("Surface Pro 8"), "sysfw-1", Some("nope"));
        assert!(matches!(r.outcome, ApplyOutcome::Refused { .. }));
    }

    #[test]
    fn armed_apply_updates_and_triggers_reverify() {
        let fake = FakeFwupd {
            apply: Ok(()),
            ..Default::default()
        };
        let r = run_apply(
            &fake,
            &detect_of("Surface Pro 8"),
            "sysfw-1",
            Some(FW_ARM_TOKEN),
        );
        assert_eq!(r.outcome, ApplyOutcome::Applied);
        assert_eq!(r.device_id, "sysfw-1");
        assert!(r.reverify, "a successful apply re-runs verify");
    }

    #[test]
    fn armed_apply_failure_is_honest_and_does_not_reverify() {
        let fake = FakeFwupd {
            apply: Err(FwError::Failed {
                action: "fwupdmgr update sysfw-1".into(),
                detail: "device rejected the update".into(),
            }),
            ..Default::default()
        };
        let r = run_apply(
            &fake,
            &detect_of("Surface Pro 8"),
            "sysfw-1",
            Some(FW_ARM_TOKEN),
        );
        assert!(matches!(r.outcome, ApplyOutcome::Failed { .. }));
        assert!(!r.reverify, "a failed apply changed nothing → no re-verify");
    }

    #[test]
    fn armed_apply_gated_live_is_honest_never_faked() {
        // The production seam must answer honestly headless — a gated apply is
        // Gated, never a fake Applied.
        let r = run_apply(
            &LiveFwupd,
            &detect_of("Surface Pro 8"),
            "sysfw-1",
            Some(FW_ARM_TOKEN),
        );
        assert!(matches!(r.outcome, ApplyOutcome::Gated { .. }));
        assert!(!r.reverify, "a gated apply did not change firmware");
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
        let r = run_apply(&FakeFwupd::default(), &det, "sysfw-1", Some(FW_ARM_TOKEN));
        assert_eq!(r.skipped.as_deref(), Some("not a Microsoft Surface"));
    }

    #[test]
    fn triggers_reverify_only_on_applied() {
        assert!(ApplyOutcome::Applied.triggers_reverify());
        assert!(!ApplyOutcome::Refused {
            reason: String::new()
        }
        .triggers_reverify());
        assert!(!ApplyOutcome::Gated {
            reason: String::new()
        }
        .triggers_reverify());
        assert!(!ApplyOutcome::Failed {
            reason: String::new()
        }
        .triggers_reverify());
    }

    #[test]
    fn live_inventory_is_integration_gated_never_faked_green() {
        // The production seam headless: honest skip, no fabricated devices.
        let inv = run_inventory(&LiveFwupd, &detect_of("Surface Pro 8"));
        assert!(inv.skipped.is_some());
        assert!(inv.devices.is_empty());
    }
}
