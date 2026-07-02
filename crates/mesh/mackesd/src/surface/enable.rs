//! SURFACE-3 — the `surface_enable` worker + guided MOK enrollment.
//!
//! The day-2 *activation* half of the Microsoft Surface enablement epic
//! (design: `docs/design/surface-tablet-enablement.md`, locks #4 + #6).
//! SURFACE-2's [`crate::surface`] detection folds the DMI identity into a
//! per-model [`SurfaceProfile`]; this unit turns that profile into the
//! concrete enablement the bootc image *can't* bake ahead of time:
//!
//! * **Activate + configure** — enable/start iptsd and apply the per-model
//!   config (Surface Aggregator perf profile, iptsd calibration, rotation
//!   /sensor hints) the recognised model needs.
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
//! is integration-gated: each live action returns an honest typed
//! [`EnableError::IntegrationGated`] rather than a faked success (§7 — the
//! same discipline `service_onboard`'s `LiveServiceApply` uses). The pure
//! core — the per-model [`plan_enable`], the [`MokState`] machine, the
//! [`run_enable`] fold — ships green and is unit-tested end-to-end against a
//! fake seam.
//!
//! This unit exposes the enable/MOK **result** types + the [`run_enable`]
//! verb reachably (SURFACE-4 publishes the enablement state to the fleet;
//! SURFACE-6 renders the Install tab from it). §6-clean: it stays wholly in
//! mackesd and reaches nothing in the desktop shell.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{Subsystem, SurfaceDetection, SurfaceDevice, SurfaceModel, SurfaceProfile};

// ─────────────────────────────── constants ──────────────────────────────────

/// The iptsd systemd unit that drives the touchscreen + pen digitizer.
pub const IPTSD_UNIT: &str = "iptsd.service";

/// The linux-surface kernel modules the verify step confirms load once the
/// MOK key is enrolled. A representative core set (touch/pen digitizer + the
/// Surface Aggregator + the HID transport); the verify board (SURFACE-4)
/// refines per subsystem.
pub const SURFACE_MODULES: &[&str] = &["surface_aggregator", "surface_hid", "hid_multitouch"];

/// Where the machine-owner key the image ships is staged for `mokutil
/// --import`. Owned by mackesd, DER-encoded (the linux-surface signing key).
pub const MOK_KEY_PATH: &str = "/var/lib/mackesd/surface/MOK.der";

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

/// The production seam. **Integration-gated** (lock #4): every live action
/// returns an honest [`EnableError::IntegrationGated`] until it's wired to a
/// real Surface at integration — never a faked success. The pure planning +
/// state machine is fully exercised behind [`SurfaceActions`] with a fake.
#[derive(Debug, Clone, Copy, Default)]
pub struct LiveSurfaceActions;

impl LiveSurfaceActions {
    fn gated<T>(action: impl Into<String>) -> Result<T, EnableError> {
        Err(EnableError::IntegrationGated {
            action: action.into(),
        })
    }
}

impl SurfaceActions for LiveSurfaceActions {
    fn enable_unit(&self, unit: &str) -> Result<bool, EnableError> {
        Self::gated(format!("enable {unit}"))
    }

    fn apply_config(&self, key: ConfigKey, _value: &str) -> Result<(), EnableError> {
        Self::gated(format!("apply {}", key.id()))
    }

    fn secure_boot_state(&self) -> Result<SecureBootState, EnableError> {
        Self::gated("read secure-boot state")
    }

    fn mok_enrolled(&self) -> Result<bool, EnableError> {
        Self::gated("query enrolled MOK keys")
    }

    fn mok_import(&self, _key_path: &Path) -> Result<String, EnableError> {
        Self::gated("mokutil --import")
    }

    fn modules_loaded(&self, _modules: &[&str]) -> Result<bool, EnableError> {
        Self::gated("verify linux-surface modules")
    }

    fn reboot(&self) -> Result<(), EnableError> {
        Self::gated("reboot to enroll MOK")
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

    // iptsd drives both the capacitive touchscreen and the active pen; enable
    // it when the model has either digitizer.
    if p.touch || p.pen {
        units.push(IPTSD_UNIT.to_string());
        configs.push(ConfigStep {
            key: ConfigKey::IptsdCalibration,
            value: "default".to_string(),
            subsystem: if p.touch {
                Subsystem::Touch
            } else {
                Subsystem::Pen
            },
        });
    }

    // Surface Aggregator perf/thermal envelope.
    if p.sam {
        configs.push(ConfigStep {
            key: ConfigKey::SamPerfProfile,
            value: "balanced".to_string(),
            subsystem: Subsystem::Sam,
        });
    }

    // Accelerometer-driven auto-rotation hint (2-in-1s only).
    if p.rotation_accel {
        configs.push(ConfigStep {
            key: ConfigKey::RotationHint,
            value: "auto".to_string(),
            subsystem: Subsystem::RotationAccel,
        });
    }

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
                // The operator typed the arm — issue the reboot (or gate it
                // honestly on a dev box).
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
pub use worker::{enable_topic, result_topic, EnableRequest, SurfaceEnableWorker};

#[cfg(feature = "async-services")]
mod worker {
    //! The per-node `surface_enable` Bus worker (a *leader-of-self* worker:
    //! it acts only on its own hardware, never a remote node). It drains
    //! [`enable_topic`] for this node, runs [`super::run_enable`] against the
    //! integration-gated [`super::LiveSurfaceActions`], and publishes the
    //! typed [`super::EnableResult`] to [`result_topic`]. SURFACE-4 folds
    //! that into the fleet enablement summary.

    use std::path::PathBuf;
    use std::time::Duration;

    use mde_bus::hooks::config::Priority;
    use mde_bus::persist::Persist;
    use serde::{Deserialize, Serialize};

    use super::{run_enable, EnableResult, LiveSurfaceActions};
    use crate::surface::{detect, SurfaceDetection};
    use crate::workers::{ShutdownToken, Worker};

    /// Poll cadence — enable is operator-driven, so a modest tick is plenty.
    pub const POLL: Duration = Duration::from_secs(2);

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

    /// The enable request envelope. `arm_token` carries the operator-typed
    /// [`super::MOK_ARM_TOKEN`] on the second (reboot-arming) call.
    #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub struct EnableRequest {
        /// The typed arming token (present only to arm the MOK reboot).
        #[serde(default)]
        pub arm_token: Option<String>,
    }

    /// The per-node `surface_enable` worker.
    pub struct SurfaceEnableWorker {
        node_id: String,
        detection: SurfaceDetection,
        bus_root: Option<PathBuf>,
        poll: Duration,
        action_cursor: Option<String>,
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
                let req: EnableRequest = msg
                    .body
                    .as_deref()
                    .and_then(|b| serde_json::from_str(b).ok())
                    .unwrap_or_default();
                let result = run_enable(
                    &LiveSurfaceActions,
                    &self.detection,
                    req.arm_token.as_deref(),
                );
                self.publish(persist, &result);
            }
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

    /// The default Bus root (same shape the other bus workers use).
    fn default_bus_root() -> Option<PathBuf> {
        Some(dirs::data_dir()?.join("mde").join("bus"))
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
                return Ok(());
            }
            let Some(root) = self.bus_root.clone() else {
                tracing::debug!(target: "mackesd::surface_enable", "no bus root; worker idle");
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
        use super::*;
        use crate::surface::{identify, DmiInfo, MS_VENDOR};

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
        fn drains_a_request_and_publishes_a_result() {
            let dir = tempfile::tempdir().expect("tempdir");
            let persist = Persist::open(dir.path().to_path_buf()).expect("open bus");
            let mut w = SurfaceEnableWorker::with_parts(
                "node-a".into(),
                detection("Surface Pro 8"),
                dir.path().to_path_buf(),
            );

            // The Install tab requests enable (no arm token).
            let req = serde_json::to_string(&EnableRequest::default()).unwrap();
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
            assert!(result.skipped.is_none());
            // Live seam is integration-gated → the MOK read is Undetermined,
            // honestly (never a faked success).
            assert!(matches!(
                result.mok,
                super::super::MokEnrollment::Undetermined { .. }
            ));
        }

        #[test]
        fn cursor_advances_so_a_request_is_processed_once() {
            let dir = tempfile::tempdir().expect("tempdir");
            let persist = Persist::open(dir.path().to_path_buf()).expect("open bus");
            let mut w = SurfaceEnableWorker::with_parts(
                "n".into(),
                detection("Surface Pro 8"),
                dir.path().to_path_buf(),
            );
            let req = serde_json::to_string(&EnableRequest::default()).unwrap();
            persist
                .write(&enable_topic("n"), Priority::Default, None, Some(&req))
                .expect("write");
            w.poll_once(&persist);
            w.poll_once(&persist); // second drain: nothing new
            let out = persist.list_since(&result_topic("n"), None).expect("list");
            assert_eq!(out.len(), 1, "request processed exactly once");
        }
    }
}

// ─────────────────────────────── tests ──────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::{identify, DmiInfo, SurfaceFamily, MS_VENDOR};

    /// A fake seam whose every action is scripted, so the fold + state
    /// machine run green without touching a machine.
    #[derive(Default)]
    struct FakeActions {
        secure_boot: Option<SecureBootState>,
        enrolled: bool,
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
            ..Default::default()
        };
        SurfaceDetection {
            model: identify(&dmi),
            dmi,
        }
    }

    fn device_of(product: &str) -> SurfaceDevice {
        match identify(&DmiInfo {
            sys_vendor: MS_VENDOR.to_string(),
            product_name: product.to_string(),
            ..Default::default()
        }) {
            SurfaceModel::Known(dev) => dev,
            other => panic!("expected Known, got {other:?}"),
        }
    }

    // ── the per-model enable plan ───────────────────────────────────────────

    #[test]
    fn plan_for_pro_enables_iptsd_sam_and_rotation() {
        let plan = plan_enable(&device_of("Surface Pro 7"));
        assert_eq!(plan.units, vec![IPTSD_UNIT.to_string()]);
        let keys: Vec<_> = plan.configs.iter().map(|c| c.key).collect();
        assert!(keys.contains(&ConfigKey::IptsdCalibration));
        assert!(keys.contains(&ConfigKey::SamPerfProfile));
        assert!(
            keys.contains(&ConfigKey::RotationHint),
            "the Pro 2-in-1 auto-rotates"
        );
    }

    #[test]
    fn plan_for_laptop_has_no_rotation_hint() {
        let plan = plan_enable(&device_of("Surface Laptop 3"));
        // Still gets iptsd (touch + pen) and SAM, but no rotation.
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
    fn sb_off_activates_and_skips_mok() {
        let fake = FakeActions {
            secure_boot: Some(SecureBootState::Disabled),
            ..Default::default()
        };
        let r = run_enable(&fake, &detect_of("Surface Pro 8"), None);
        assert_eq!(r.model, "Surface Pro 8");
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
        let r = run_enable(&fake, &detect_of("Surface Pro 8"), None);
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
            ..Default::default()
        };
        let r = run_enable(&fake, &detect_of("Surface Pro 8"), Some(MOK_ARM_TOKEN));
        assert_eq!(
            r.mok,
            MokEnrollment::RebootArmed {
                outcome: StepOutcome::Applied
            }
        );
    }

    #[test]
    fn wrong_arm_token_does_not_reboot_it_stages() {
        let fake = FakeActions {
            secure_boot: Some(SecureBootState::Enabled),
            enrolled: false,
            ..Default::default()
        };
        let r = run_enable(&fake, &detect_of("Surface Pro 8"), Some("nope"));
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
        let r = run_enable(&fake, &detect_of("Surface Pro 8"), None);
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
        let r = run_enable(&fake, &detect_of("Surface Pro 8"), None);
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
        let r = run_enable(&fake, &detect_of("Surface Pro 8"), None);
        assert!(matches!(r.mok, MokEnrollment::Undetermined { .. }));
    }

    #[test]
    fn a_failed_unit_is_recorded_as_failed_not_dropped() {
        let fake = FakeActions {
            secure_boot: Some(SecureBootState::Disabled),
            enable_fails: true,
            ..Default::default()
        };
        let r = run_enable(&fake, &detect_of("Surface Pro 8"), None);
        assert!(matches!(
            r.activation.units[0].outcome,
            StepOutcome::Failed { .. }
        ));
    }

    #[test]
    fn live_seam_is_integration_gated_never_faked() {
        // The production seam must answer honestly, never a green lie.
        let r = run_enable(&LiveSurfaceActions, &detect_of("Surface Pro 8"), None);
        assert!(matches!(
            r.activation.units[0].outcome,
            StepOutcome::Gated { .. }
        ));
        assert!(matches!(r.mok, MokEnrollment::Undetermined { .. }));
    }
}
