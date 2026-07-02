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
//! The production seam ([`LiveSurfaceProbes`]) reads `/sys` / evdev
//! directly (§9 — no `dmidecode`/shell), and the confirmations a headless
//! box genuinely can't make safely (a camera frame grab, a fingerprint
//! enroll capability) come back as an honest [`ProbeError::IntegrationGated`]
//! rather than a faked success (§7 — the same discipline
//! [`super::enable::LiveSurfaceActions`] uses). Interactive-gesture probes
//! (pen pressure/tilt, S0ix suspend residency) fold to
//! [`ProbeState::NeedsGesture`] — an honest operator prompt, not a fault.
//! The pure classification folds (reading → tri-state) are unit-tested with
//! fixtures; the live reads are integration-gated.
//!
//! Alongside the full board this unit publishes the **compact
//! `state/hardware/surface/<node>` summary** (model, enablement %, count of
//! red subsystems) the Controller/fleet rollup reads (lock #7 — visibility
//! only, never remote control). §6-clean: it stays wholly in mackesd.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{Subsystem, SurfaceDetection, SurfaceDevice, SurfaceModel, SurfaceProfile};

// ─────────────────────────────── the tri-state ──────────────────────────────

/// One subsystem's verify verdict — the board's cell state (lock #5).
///
/// The tri-state is `Ok`/`Degraded`/`Failed`; the interactive-gesture probes
/// (pen, suspend) add [`Self::NeedsGesture`], which prompts the operator
/// honestly rather than faulting a subsystem we simply haven't exercised yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProbeState {
    /// Verified working — the reading confirms the subsystem is live.
    Ok,
    /// Present but not fully healthy — one of several expected signals is
    /// missing (SAM battery readable but thermal not; pen pressure but no
    /// tilt; a camera enumerated but no frame confirmed).
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

/// The camera reading — device enumerated + whether a frame was captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CameraReading {
    /// A V4L2 capture device is enumerated.
    pub device_present: bool,
    /// A frame was actually captured (the live confirmation).
    pub frame_captured: bool,
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

/// The fingerprint reader reading — device present + enroll capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FingerprintReading {
    /// A fingerprint device is enumerated.
    pub device_present: bool,
    /// The device reports it can enroll (driver + stack ready).
    pub enroll_capable: bool,
}

// ─────────────────────────────── the seam ───────────────────────────────────

/// A typed failure from the [`SurfaceProbes`] seam — mirrors
/// [`super::enable::EnableError`]'s honest split.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
/// Each probe returns [`ProbeError::IntegrationGated`] when the live read is
/// integration-gated (headless / non-Surface) and [`ProbeError::Failed`] on a
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
    /// Read the camera (enumeration + a frame capture).
    fn probe_camera(&self) -> Result<CameraReading, ProbeError>;
    /// Read the Wi-Fi + Bluetooth radios' up/down state.
    fn probe_wifi_bt(&self) -> Result<WifiBtReading, ProbeError>;
    /// Read the S0ix residency counter (advancement needs a suspend gesture).
    fn probe_s0ix(&self) -> Result<S0ixReading, ProbeError>;
    /// Read the fingerprint reader (presence + enroll capability).
    fn probe_fingerprint(&self) -> Result<FingerprintReading, ProbeError>;
}

// ─────────────────────────── the production seam ────────────────────────────

/// The production seam. §9-clean: it reads `/sys` / evdev directly (no
/// `dmidecode`/shell).
///
/// The confirmations a headless box genuinely can't
/// make safely — a camera **frame grab** and a fingerprint **enroll
/// capability** query — return an honest [`ProbeError::IntegrationGated`]
/// rather than a faked success (lock #5, §7). The presence-style reads are
/// real; the interactive fields (pen pressure/tilt, S0ix advancement) come
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
    const V4L_DIR: &'static str = "/sys/class/video4linux";
    /// A representative Intel PMC S0ix residency counter (µs since boot).
    const S0IX_RESIDENCY: &'static str = "/sys/kernel/debug/pmc_core/slp_s0_residency_usec";

    fn gated<T>(probe: impl Into<String>) -> Result<T, ProbeError> {
        Err(ProbeError::IntegrationGated {
            probe: probe.into(),
        })
    }

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
        // Enumeration is a §9 file read; actually opening the V4L2 device and
        // grabbing a frame is a live interaction a headless box can't safely
        // do — gate the whole probe honestly rather than claim a green frame.
        if Self::any_dir_entry(Self::V4L_DIR) {
            Self::gated("camera frame capture")
        } else {
            Ok(CameraReading {
                device_present: false,
                frame_captured: false,
            })
        }
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
        // Enroll capability lives behind the fprint/libfprint userspace stack,
        // not a §9 sysfs scalar — gate it honestly rather than fake a green.
        Self::gated("fingerprint enroll capability")
    }
}

// ─────────────────────────── the classification folds (pure) ────────────────

/// One subsystem's row on the verify board — the subsystem, its tri-state,
/// and the real reason string (lock #5: every cell carries a reason).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// Classify the camera reading. A captured frame → green; enumerated but no
/// frame → degraded; absent → red. (Live gates the frame grab honestly.)
#[must_use]
pub fn classify_camera(reading: Result<CameraReading, ProbeError>) -> SubsystemVerdict {
    let r = match reading {
        Ok(r) => r,
        Err(e) => return SubsystemVerdict::from_err(Subsystem::Cameras, &e),
    };
    let (state, reason) = if !r.device_present {
        (ProbeState::Failed, "no camera enumerated".to_string())
    } else if r.frame_captured {
        (ProbeState::Ok, "camera captured a frame".to_string())
    } else {
        (
            ProbeState::Degraded,
            "camera enumerated but no frame captured".to_string(),
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

/// Classify the fingerprint reading. Present + enroll-capable → green;
/// present but not capable → degraded; absent → red. (Live gates the
/// capability query honestly.)
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
    } else if r.enroll_capable {
        (
            ProbeState::Ok,
            "fingerprint reader ready to enroll".to_string(),
        )
    } else {
        (
            ProbeState::Degraded,
            "fingerprint reader present but not enroll-capable".to_string(),
        )
    };
    SubsystemVerdict::new(Subsystem::Fingerprint, state, reason)
}

// ─────────────────────────── the board (profile-gated) ──────────────────────

/// The full per-node verify board — the model string + one row per subsystem
/// the model's profile claims. SURFACE-6's Test tab renders it; [`summarize`]
/// folds it to the compact fleet summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

// ─────────────────────────── the Bus worker (per-node) ──────────────────────

#[cfg(feature = "async-services")]
pub use worker::{board_topic, summary_topic, SurfaceVerifyWorker};

#[cfg(feature = "async-services")]
mod worker {
    //! The per-node `surface_verify` Bus worker (a *leader-of-self* worker:
    //! it probes only its own hardware, never a remote node). Each tick it
    //! runs [`super::run_verify`] against the integration-gated
    //! [`super::LiveSurfaceProbes`], publishes the full board to
    //! [`board_topic`] (SURFACE-6's Test tab), and the compact
    //! [`super::FleetSummary`] to [`summary_topic`] (the fleet rollup, lock
    //! #7). On a non-Surface node it idles (never touches the Bus).

    use std::path::PathBuf;
    use std::time::Duration;

    use mde_bus::hooks::config::Priority;
    use mde_bus::persist::Persist;

    use super::{run_verify, summarize, LiveSurfaceProbes};
    use crate::surface::{detect, SurfaceDetection};
    use crate::workers::{ShutdownToken, Worker};

    /// Re-verify cadence — the board is fleet-visibility, not hot-path, so a
    /// modest tick keeps the rollup fresh without churn.
    pub const POLL: Duration = Duration::from_secs(30);

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

    /// The per-node `surface_verify` worker.
    pub struct SurfaceVerifyWorker {
        node_id: String,
        detection: SurfaceDetection,
        bus_root: Option<PathBuf>,
        poll: Duration,
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
            }
        }

        /// Test constructor: an explicit detection + bus root, no real /sys.
        #[cfg(test)]
        #[must_use]
        pub(crate) const fn with_parts(
            node_id: String,
            detection: SurfaceDetection,
            bus_root: PathBuf,
        ) -> Self {
            Self {
                node_id,
                detection,
                bus_root: Some(bus_root),
                poll: POLL,
            }
        }

        /// Probe once and publish the board + the compact summary. Pulled out
        /// so a test drives it against a temp Bus without the run loop/clock.
        fn probe_once(&self, persist: &Persist) {
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
                target: "mackesd::surface_verify",
                topic,
                error = %e,
                "verify publish failed"
            );
        }
    }

    /// The default Bus root (same shape the other bus workers use).
    fn default_bus_root() -> Option<PathBuf> {
        Some(dirs::data_dir()?.join("mde").join("bus"))
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
                return Ok(());
            }
            let Some(root) = self.bus_root.clone() else {
                tracing::debug!(target: "mackesd::surface_verify", "no bus root; worker idle");
                return Ok(());
            };
            loop {
                match Persist::open(root.clone()) {
                    Ok(persist) => self.probe_once(&persist),
                    Err(e) => tracing::debug!(
                        target: "mackesd::surface_verify",
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
        use crate::surface::verify::{FleetSummary, VerifyBoard};
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
        fn publishes_board_and_summary_for_a_surface() {
            let dir = tempfile::tempdir().expect("tempdir");
            let persist = Persist::open(dir.path().to_path_buf()).expect("open bus");
            let w = SurfaceVerifyWorker::with_parts(
                "node-a".into(),
                detection("Surface Pro 8"),
                dir.path().to_path_buf(),
            );

            w.probe_once(&persist);

            let boards = persist
                .list_since(&board_topic("node-a"), None)
                .expect("list boards");
            assert_eq!(boards.len(), 1, "one board published");
            let board: VerifyBoard =
                serde_json::from_str(boards[0].body.as_deref().unwrap()).unwrap();
            assert_eq!(board.model, "Surface Pro 8");
            assert!(!board.rows.is_empty(), "the Pro claims subsystems");

            let summaries = persist
                .list_since(&summary_topic("node-a"), None)
                .expect("list summaries");
            assert_eq!(summaries.len(), 1, "one summary published");
            let summary: FleetSummary =
                serde_json::from_str(summaries[0].body.as_deref().unwrap()).unwrap();
            assert_eq!(summary.node, "node-a");
            assert_eq!(summary.model, "Surface Pro 8");
            // Live seam is integration-gated headless → nothing fully green,
            // so enablement is honestly 0% (never a faked green).
            assert_eq!(summary.enablement_pct, 0);
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
                    frame_captured: true,
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
                    enroll_capable: true,
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
    fn fingerprint_present_but_incapable_is_degraded() {
        assert_eq!(
            classify_fingerprint(Ok(FingerprintReading {
                device_present: true,
                enroll_capable: true
            }))
            .state,
            ProbeState::Ok
        );
        assert_eq!(
            classify_fingerprint(Ok(FingerprintReading {
                device_present: true,
                enroll_capable: false
            }))
            .state,
            ProbeState::Degraded
        );
        assert_eq!(
            classify_fingerprint(Ok(FingerprintReading {
                device_present: false,
                enroll_capable: false
            }))
            .state,
            ProbeState::Failed
        );
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
    fn live_probes_are_integration_gated_never_faked_green() {
        // The production seam must answer honestly headless — the gated
        // camera + fingerprint fold to red, nothing is a green lie.
        let board = run_verify(&LiveSurfaceProbes, &detect_of("Surface Laptop 3"));
        let camera = board
            .rows
            .iter()
            .find(|r| r.subsystem == Subsystem::Cameras)
            .expect("camera row");
        // Camera is either absent (no v4l dir) or gated — both are red, never
        // a fake green.
        assert_eq!(camera.state, ProbeState::Failed);
        let fp = board
            .rows
            .iter()
            .find(|r| r.subsystem == Subsystem::Fingerprint)
            .expect("fingerprint row");
        assert_eq!(fp.state, ProbeState::Failed);
        assert!(fp.reason.contains("integration-gated"));
    }
}
