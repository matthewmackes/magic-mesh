//! SURFACE-6 — the This Node **"Surface / Hardware Enablement"** card.
//!
//! The epic's UI closer: a **model-gated** card mounted inside the This Node
//! plane that renders the SURFACE-2/3/4/5/7 backend the `mackesd` surface
//! workers publish, and drives their typed verbs. Three tabs (design lock #10):
//!
//! * **Install** — activate/enable (the `surface_enable` verb), the guided MOK
//!   enrollment flow (the [`MokEnrollment`] state + the typed-armed reboot
//!   control + the firmware-prompt copy), and the SURFACE-5 fwupd firmware list
//!   with a typed-armed `fw-apply` control.
//! * **Test** — the SURFACE-4 tri-state probe board (each subsystem
//!   Ok/Failed/Degraded/NeedsGesture with its reason) + a re-read control.
//! * **Config** — the applied per-model config knobs (read from the enable
//!   result), the seat formfactor note, and the SURFACE-7 DRM mode picker +
//!   fractional scale (the in-process [`DisplayController`]).
//!
//! ## One wire contract, no daemon dependency (§6 glue)
//!
//! Like the other Bus-backed provisioning surfaces, this module
//! leans inward only on `mde-bus` and the shared bounded Surface contract: it
//! **reads** the typed state the workers
//! publish under `state/hardware/surface/<node>/*` and **publishes** the typed
//! requests they drain under `action/hardware/surface/<node>/*`. The `<node>`
//! id is discovered from the Bus itself — the summary topic a Surface node
//! publishes IS the model gate: no summary ⇒ not a Surface ⇒ the card never
//! appears (design lock #3/#7).
//!
//! ## Honest by construction (§7)
//!
//! Every field is the worker's real typed state, rendered as-is: an
//! integration-gated enable step shows as gated, a `NeedsGesture` probe prompts
//! the operator, and a shipped DRM modeset is acknowledged only after the sole
//! seat runner rebuilt GBM/EGL and committed KMS — never a faked success. With no Bus
//! (or no Surface) on the box the card simply isn't drawn.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mackes_mesh_types::surface_hardware::{
    SurfaceActionHeader, SurfaceAvailability, SurfaceFirmwareApplyRequest,
    SurfaceFirmwareInventory, SurfaceFleetSummary, SurfaceProbeState, SurfaceSubsystem,
    SurfaceVerifyBoard, SURFACE_HARDWARE_SCHEMA_VERSION,
};
use mde_egui::egui::{self, RichText};
use mde_egui::{DisplayController, ModeClass, ModesetDispatch, PanelInfo, Style};
use serde::Deserialize;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use crate::bus_reader::BusReader;

/// Poll cadence — the surface workers publish on 2–30 s ticks, so a modest read
/// cadence keeps the card fresh without spinning. Matches the This Node plane.
const REFRESH: Duration = Duration::from_secs(5);
/// A verifier publishes every 30 seconds. Three missed publications is the
/// bounded point where a previously fresh observation becomes visibly stale.
const MAX_SURFACE_STATE_AGE_MS: u64 = 90_000;
/// Tolerated publisher/seat wall-clock skew before a future state is refused.
const MAX_SURFACE_STATE_FUTURE_SKEW_MS: u64 = 5_000;

/// The exact token the operator types to arm a firmware apply (mirror of
/// `mackesd::surface::firmware::FW_ARM_TOKEN`, lock #8).
const FW_ARM_TOKEN: &str = "APPLY-SURFACE-FIRMWARE";

// ─────────────────────────── the topic helpers (§6) ─────────────────────────

/// The compact fleet summary lane — its presence IS the model gate.
fn summary_topic(node: &str) -> String {
    format!("state/hardware/surface/{node}")
}
/// The full tri-state probe board lane (Test tab).
fn board_topic(node: &str) -> String {
    format!("state/hardware/surface/{node}/probes")
}
/// The typed enable-result lane (Install tab).
fn enable_result_topic(node: &str) -> String {
    format!("state/hardware/surface/{node}/enable")
}
/// The fwupd inventory lane (Install tab).
fn firmware_topic(node: &str) -> String {
    format!("state/hardware/surface/{node}/firmware")
}
/// The fw-apply request lane (Install tab).
fn fw_apply_action_topic(node: &str) -> String {
    format!("action/hardware/surface/{node}/fw-apply")
}
/// The fw-apply typed-result lane (Install tab).
fn fw_apply_result_topic(node: &str) -> String {
    format!("state/hardware/surface/{node}/fw-apply")
}

// ───────────────────────── the wire mirrors — state (§6) ────────────────────

/// Mirror of the daemon's `Subsystem` (default enum repr — the variant name on
/// the wire). Carries the human label the board renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
enum Subsystem {
    Touch,
    Pen,
    TypeCover,
    Sam,
    RotationAccel,
    Cameras,
    WifiBt,
    S0ix,
    Fingerprint,
}

fn probe_tone(state: SurfaceProbeState) -> egui::Color32 {
    match state {
        SurfaceProbeState::Ok => Style::OK,
        SurfaceProbeState::Degraded => Style::WARN,
        SurfaceProbeState::Failed => Style::DANGER,
        SurfaceProbeState::NeedsGesture => Style::ACCENT,
    }
}

fn probe_word(state: SurfaceProbeState) -> &'static str {
    match state {
        SurfaceProbeState::Ok => "ok",
        SurfaceProbeState::Degraded => "degraded",
        SurfaceProbeState::Failed => "failed",
        SurfaceProbeState::NeedsGesture => "needs gesture",
    }
}

fn shared_subsystem_label(subsystem: SurfaceSubsystem) -> &'static str {
    match subsystem {
        SurfaceSubsystem::Touch => "Touchscreen",
        SurfaceSubsystem::Pen => "Pen / stylus",
        SurfaceSubsystem::TypeCover => "Type Cover",
        SurfaceSubsystem::Sam => "Surface Aggregator (battery/thermal)",
        SurfaceSubsystem::RotationAccel => "Auto-rotation (accelerometer)",
        SurfaceSubsystem::Cameras => "Cameras",
        SurfaceSubsystem::WifiBt => "Wi-Fi / Bluetooth",
        SurfaceSubsystem::S0ix => "S0ix suspend",
        SurfaceSubsystem::Fingerprint => "Fingerprint reader",
    }
}

fn shared_subsystem_id(subsystem: SurfaceSubsystem) -> &'static str {
    match subsystem {
        SurfaceSubsystem::Touch => "touch",
        SurfaceSubsystem::Pen => "pen",
        SurfaceSubsystem::TypeCover => "type_cover",
        SurfaceSubsystem::Sam => "sam",
        SurfaceSubsystem::RotationAccel => "rotation_accel",
        SurfaceSubsystem::Cameras => "cameras",
        SurfaceSubsystem::WifiBt => "wifi_bt",
        SurfaceSubsystem::S0ix => "s0ix",
        SurfaceSubsystem::Fingerprint => "fingerprint",
    }
}

/// Mirror of the daemon's `ConfigKey`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
enum ConfigKey {
    SamPerfProfile,
    IptsdCalibration,
    RotationHint,
}

impl ConfigKey {
    const fn label(self) -> &'static str {
        match self {
            Self::SamPerfProfile => "SAM perf profile",
            Self::IptsdCalibration => "iptsd calibration / sensitivity",
            Self::RotationHint => "Rotation hint",
        }
    }
}

/// Mirror of the daemon's `StepOutcome` (externally-tagged).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
enum StepOutcome {
    Applied,
    AlreadyActive,
    Gated { reason: String },
    Failed { reason: String },
}

impl StepOutcome {
    const fn tone(&self) -> egui::Color32 {
        match self {
            Self::Applied | Self::AlreadyActive => Style::OK,
            Self::Gated { .. } => Style::WARN,
            Self::Failed { .. } => Style::DANGER,
        }
    }
    fn summary(&self) -> String {
        match self {
            Self::Applied => "applied".to_string(),
            Self::AlreadyActive => "already active".to_string(),
            Self::Gated { reason } => format!("integration-gated \u{2014} {reason}"),
            Self::Failed { reason } => format!("failed \u{2014} {reason}"),
        }
    }
}

/// Mirror of the daemon's `UnitResult`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct UnitResult {
    unit: String,
    outcome: StepOutcome,
}

/// Mirror of the daemon's `ConfigResult`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct ConfigResult {
    key: ConfigKey,
    #[allow(dead_code)]
    subsystem: Subsystem,
    outcome: StepOutcome,
}

/// Mirror of the daemon's `ActivationResult`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
struct ActivationResult {
    #[serde(default)]
    units: Vec<UnitResult>,
    #[serde(default)]
    configs: Vec<ConfigResult>,
}

/// Mirror of the daemon's `MokEnrollment` state machine (SURFACE-3, lock #6).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
enum MokEnrollment {
    NotRequired,
    Enrolled {
        modules_loaded: bool,
    },
    ImportedAwaitingArm {
        firmware_prompt: String,
        arm_token: String,
        key_fingerprint: String,
    },
    RebootArmed {
        outcome: StepOutcome,
    },
    Undetermined {
        reason: String,
    },
}

/// Mirror of the daemon's `EnableResult` (SURFACE-3, Install tab).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct EnableResult {
    model: String,
    #[serde(default)]
    skipped: Option<String>,
    #[serde(default)]
    activation: ActivationResult,
    mok: MokEnrollment,
}

/// Mirror of the daemon's `ApplyOutcome`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
enum ApplyOutcome {
    Refused { reason: String },
    Applied,
    Gated { reason: String },
    Failed { reason: String },
}

/// Mirror of the daemon's `ApplyResult` (SURFACE-5 fw-apply).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct ApplyResult {
    #[allow(dead_code)]
    model: String,
    #[serde(default)]
    skipped: Option<String>,
    device_id: String,
    outcome: ApplyOutcome,
    #[allow(dead_code)]
    reverify: bool,
}

// ───────────────────────── shared action contract (§6) ─────────────────────

fn surface_action_header(node: &str) -> SurfaceActionHeader {
    static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);
    let issued_at_ms: u64 = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);
    let sequence = REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    SurfaceActionHeader {
        schema_version: SURFACE_HARDWARE_SCHEMA_VERSION,
        node: node.to_string(),
        request_id: format!("surface-{issued_at_ms}-{sequence}"),
        issued_at_ms,
        armed_token: None,
    }
}

// ──────────────────────────── the card state ────────────────────────────────

/// Which tab of the card is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Install,
    Test,
    Config,
}

impl Tab {
    const ALL: [Self; 3] = [Self::Install, Self::Test, Self::Config];
    const fn label(self) -> &'static str {
        match self {
            Self::Install => "Install",
            Self::Test => "Test",
            Self::Config => "Config",
        }
    }
}

/// The Surface / Hardware Enablement card's live state: the discovered node id,
/// the typed worker state read off the Bus (the summary is the model gate), the
/// operator's in-flight typed-arm inputs, and the in-process display controller.
pub(crate) struct SurfaceCardState {
    /// Desktop-client Bus spool (resolved once). `None` on a box with no Bus dir
    /// — the card can't be gated on, so it never appears.
    bus_root: Option<PathBuf>,
    /// The discovered `<node>` id (from the summary topic) — also the key the
    /// per-node action/result topics are built from.
    node: Option<String>,
    /// The compact fleet summary — `Some` IS the model gate (a Surface node
    /// published it; a non-Surface node never does).
    summary: Option<SurfaceFleetSummary>,
    /// The full tri-state probe board (Test tab).
    board: Option<SurfaceVerifyBoard>,
    /// The typed enable result (Install tab).
    enable: Option<EnableResult>,
    /// The fwupd inventory (Install tab).
    firmware: Option<SurfaceFirmwareInventory>,
    /// The last fw-apply typed result (Install tab).
    apply: Option<ApplyResult>,
    /// The showing tab.
    tab: Tab,
    /// The operator's typed firmware arm token.
    fw_arm_input: String,
    /// The firmware device the operator selected to apply.
    selected_fw: Option<String>,
    /// A transient "request sent" note surfaced until the next state update.
    action_note: Option<String>,
    /// The last publish error, surfaced inline (honest; never a panic).
    last_error: Option<String>,
    /// The in-process display controller (SURFACE-7). Built from the panel EDID
    /// when readable; `None` on a headless/farm box (then the Config tab shows
    /// the live egui scale + an honest note).
    display: Option<DisplayController>,
    /// The last modeset attempt's honest outcome message (gated / applied).
    modeset_note: Option<String>,
    // read cursors (only new messages are decoded; the latest wins).
    cur_summary: Option<String>,
    cur_board: Option<String>,
    cur_enable: Option<String>,
    cur_firmware: Option<String>,
    cur_apply: Option<String>,
    /// When the Bus was last polled (drives the fixed cadence).
    last_poll: Option<Instant>,
}

impl Default for SurfaceCardState {
    fn default() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
            node: None,
            summary: None,
            board: None,
            enable: None,
            firmware: None,
            apply: None,
            tab: Tab::default(),
            fw_arm_input: String::new(),
            selected_fw: None,
            action_note: None,
            last_error: None,
            display: probe_panel().map(production_display_controller),
            modeset_note: None,
            cur_summary: None,
            cur_board: None,
            cur_enable: None,
            cur_firmware: None,
            cur_apply: None,
            last_poll: None,
        }
    }
}

/// Read the first parseable panel EDID from `/sys/class/drm/*/edid` (real
/// hardware, best-effort). Only the native mode + physical size are known from
/// the base block; the full connector mode list arrives when the DRM runner
/// injects a connector-derived controller. `None` on a headless/farm box.
fn probe_panel() -> Option<PanelInfo> {
    let entries = std::fs::read_dir(Path::new("/sys/class/drm")).ok()?;
    for entry in entries.flatten() {
        let edid_path = entry.path().join("edid");
        let Ok(bytes) = std::fs::read(&edid_path) else {
            continue;
        };
        if bytes.is_empty() {
            continue;
        }
        if let Ok(panel) = mde_egui::parse_edid(&bytes) {
            return Some(PanelInfo::new(panel.native, panel.phys_mm, &[panel.native]));
        }
    }
    None
}

fn production_display_controller(panel: PanelInfo) -> DisplayController {
    #[cfg(feature = "drm")]
    {
        DisplayController::runner(panel)
    }
    #[cfg(not(feature = "drm"))]
    {
        DisplayController::headless(panel)
    }
}

impl SurfaceCardState {
    /// `true` when a Surface was detected on this node (the model gate). The
    /// workbench only draws the card when this holds (design lock #3/#7).
    pub(crate) const fn is_surface(&self) -> bool {
        self.summary.is_some()
    }

    /// The poll seam: on the fixed cadence, discover the node id (if not yet
    /// known) and re-read the latest typed worker state off the Bus, then keep
    /// the repaint heartbeat alive so a fresh board / enable result surfaces
    /// without operator input. Cheap per frame — it self-gates.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            self.refresh();
        }
        ctx.request_repaint_after(REFRESH);
    }

    /// Bind the exact local node + re-read all state lanes. A missing local
    /// summary clears retained state; a remote Surface can never become This Node.
    fn refresh(&mut self) {
        // arch-11: open through the shared BusReader seam.
        let Some(persist) = BusReader::new(self.bus_root.clone()).open() else {
            self.clear_surface_state();
            return;
        };
        let local_node = crate::explorer::local_hostname();
        if discover_node(&persist, &local_node).is_none() {
            self.clear_surface_state();
            return;
        }
        if self.node.as_deref() != Some(local_node.as_str()) {
            self.clear_surface_state();
            self.node = Some(local_node);
        }
        let Some(node) = self.node.clone() else {
            return;
        };
        read_latest_surface_summary(
            &persist,
            &summary_topic(&node),
            &node,
            &mut self.cur_summary,
            &mut self.summary,
        );
        read_latest_surface_board(
            &persist,
            &board_topic(&node),
            &node,
            &mut self.cur_board,
            &mut self.board,
        );
        read_latest(
            &persist,
            &enable_result_topic(&node),
            &mut self.cur_enable,
            &mut self.enable,
        );
        read_latest_surface_firmware(
            &persist,
            &firmware_topic(&node),
            &node,
            &mut self.cur_firmware,
            &mut self.firmware,
        );
        read_latest(
            &persist,
            &fw_apply_result_topic(&node),
            &mut self.cur_apply,
            &mut self.apply,
        );
        // Re-age retained facts on every poll as well as on decode. Otherwise
        // a last-good `Fresh` value would stay cosmetically fresh forever when
        // the producer stops publishing and the cursor sees no new messages.
        let now_ms = wall_clock_ms();
        if let Some(summary) = self.summary.as_mut() {
            let _ = admit_publication_freshness(&mut summary.publication, &node, now_ms);
        }
        if let Some(board) = self.board.as_mut() {
            let _ = admit_publication_freshness(&mut board.publication, &node, now_ms);
        }
        if let Some(firmware) = self.firmware.as_mut() {
            let _ = admit_publication_freshness(&mut firmware.publication, &node, now_ms);
        }
    }

    fn clear_surface_state(&mut self) {
        self.node = None;
        self.summary = None;
        self.board = None;
        self.enable = None;
        self.firmware = None;
        self.apply = None;
        self.cur_summary = None;
        self.cur_board = None;
        self.cur_enable = None;
        self.cur_firmware = None;
        self.cur_apply = None;
    }

    /// Force an immediate re-read (the Test tab's re-read control + used after a
    /// publish so the fresh state surfaces on the next frame). Honest: it
    /// re-reads the Bus — the node re-verifies on its own 30 s tick.
    const fn force_refresh(&mut self) {
        self.last_poll = None;
    }

    /// Publish a typed action body to `topic`, recording an honest note / error.
    fn publish(&mut self, topic: &str, body: &str, note: &str) {
        let Some(root) = self.bus_root.clone() else {
            self.last_error = Some("No mesh Bus \u{2014} can't send the request.".to_string());
            return;
        };
        // arch-11: writer — the shared BusReader seam is read-only; this publish
        // keeps Persist::open because it needs the write Result to set `last_error`.
        match Persist::open(root).and_then(|p| p.write(topic, Priority::Default, None, Some(body)))
        {
            Ok(_) => {
                self.last_error = None;
                self.action_note = Some(note.to_string());
                self.force_refresh();
            }
            Err(e) => self.last_error = Some(format!("Couldn't send the request: {e}")),
        }
    }

    /// Render the card into `ui`. The caller (the This Node plane) only reaches
    /// here when [`is_surface`](Self::is_surface) holds, so the model gate is
    /// enforced one level up; this still no-ops defensively without a summary.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        let Some(summary) = self.summary.clone() else {
            return;
        };

        ui.add_space(Style::SP_M);
        ui.separator();
        ui.add_space(Style::SP_S);

        // ── header: the model + the enablement rollup (real, lock #7) ──
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("Surface / Hardware Enablement")
                    .color(Style::TEXT)
                    .size(Style::BODY)
                    .strong(),
            );
        });
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(
                Style::ACCENT,
                RichText::new(&summary.publication.model.product).size(Style::SMALL),
            );
            ui.add_space(Style::SP_S);
            let tone = if summary.red_count > 0 {
                Style::DANGER
            } else if summary.enablement_pct == 100 {
                Style::OK
            } else {
                Style::WARN
            };
            ui.colored_label(
                tone,
                RichText::new(format!("{}% enabled", summary.enablement_pct)).size(Style::SMALL),
            );
            if summary.red_count > 0 {
                ui.add_space(Style::SP_S);
                ui.colored_label(
                    Style::DANGER,
                    RichText::new(format!(
                        "{} red: {}",
                        summary.red_count,
                        summary
                            .red_subsystems
                            .iter()
                            .copied()
                            .map(shared_subsystem_id)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                    .size(Style::SMALL),
                );
            }
        });
        match &summary.publication.availability {
            SurfaceAvailability::Fresh => {
                mde_egui::muted_note(
                    ui,
                    format!(
                        "Fresh local {:?} observation · published {}",
                        summary.publication.source, summary.publication.published_at_ms
                    ),
                );
            }
            SurfaceAvailability::Stale { reason } => {
                mde_egui::muted_note(ui, format!("Stale hardware state: {reason}"));
            }
            SurfaceAvailability::Unavailable { reason } => {
                ui.colored_label(
                    Style::WARN,
                    RichText::new(format!("Hardware state unavailable: {reason}"))
                        .size(Style::SMALL),
                );
            }
        }
        ui.add_space(Style::SP_S);

        if let Some(err) = self.last_error.clone() {
            ui.colored_label(Style::DANGER, err);
            ui.add_space(Style::SP_XS);
        }

        // ── tab bar ──
        ui.horizontal_wrapped(|ui| {
            for tab in Tab::ALL {
                if ui.selectable_label(self.tab == tab, tab.label()).clicked() {
                    self.tab = tab;
                }
                ui.add_space(Style::SP_XS);
            }
        });
        ui.add_space(Style::SP_S);

        match self.tab {
            Tab::Install => self.show_install(ui),
            Tab::Test => self.show_test(ui),
            Tab::Config => self.show_config(ui),
        }

        if let Some(note) = self.action_note.clone() {
            ui.add_space(Style::SP_S);
            mde_egui::muted_note(ui, note);
        }
    }

    // ─────────────────────────── Install tab ───────────────────────────

    fn show_install(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(
                Style::ACCENT,
                RichText::new("GOVERNED ACTIVATION")
                    .size(Style::SMALL)
                    .strong(),
            );
            mde_egui::muted_note(ui, "Local exact-body authority · audited result below");
        });
        ui.add_space(Style::SP_S);

        // The typed enable result + the guided MOK flow.
        match self.enable.clone() {
            Some(res) if res.skipped.is_none() => self.show_enable_result(ui, &res),
            Some(res) => {
                mde_egui::muted_note(
                    ui,
                    format!(
                        "Enable skipped: {}",
                        res.skipped.unwrap_or_else(|| "unknown".to_string())
                    ),
                );
            }
            None => {
                mde_egui::muted_note(
                    ui,
                    "No activation result yet. Surface controls remain unchanged.",
                );
            }
        }

        ui.add_space(Style::SP_M);
        ui.separator();
        ui.add_space(Style::SP_S);
        ui.label(
            RichText::new("FIRMWARE (fwupd)")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL)
                .strong(),
        );
        ui.add_space(Style::SP_XS);
        self.show_firmware(ui);
    }

    fn show_enable_result(&mut self, ui: &mut egui::Ui, res: &EnableResult) {
        // Activation units.
        for unit in &res.activation.units {
            ui.horizontal_wrapped(|ui| {
                mde_egui::status_dot(ui, unit.outcome.tone());
                ui.add_space(Style::SP_XS);
                ui.label(
                    RichText::new(&unit.unit)
                        .color(Style::TEXT)
                        .size(Style::SMALL),
                );
                ui.add_space(Style::SP_S);
                mde_egui::muted_note(ui, unit.outcome.summary());
            });
        }
        ui.add_space(Style::SP_S);

        // The guided MOK enrollment flow (lock #6).
        ui.label(
            RichText::new("Secure Boot / MOK")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL)
                .strong(),
        );
        ui.add_space(Style::SP_XS);
        match &res.mok {
            MokEnrollment::NotRequired => {
                ui.horizontal_wrapped(|ui| {
                    mde_egui::status_dot(ui, Style::OK);
                    ui.add_space(Style::SP_XS);
                    mde_egui::muted_note(ui, "Secure Boot off \u{2014} no key to enroll.");
                });
            }
            MokEnrollment::Enrolled { modules_loaded } => {
                let tone = if *modules_loaded {
                    Style::OK
                } else {
                    Style::WARN
                };
                ui.horizontal_wrapped(|ui| {
                    mde_egui::status_dot(ui, tone);
                    ui.add_space(Style::SP_XS);
                    mde_egui::muted_note(
                        ui,
                        if *modules_loaded {
                            "Key enrolled \u{2014} linux-surface modules load."
                        } else {
                            "Key enrolled, but the linux-surface modules aren't loaded yet."
                        },
                    );
                });
            }
            MokEnrollment::RebootArmed { outcome } => {
                ui.horizontal_wrapped(|ui| {
                    mde_egui::status_dot(ui, outcome.tone());
                    ui.add_space(Style::SP_XS);
                    mde_egui::muted_note(
                        ui,
                        format!("Reboot armed \u{2014} {}", outcome.summary()),
                    );
                });
            }
            MokEnrollment::Undetermined { reason } => {
                ui.horizontal_wrapped(|ui| {
                    mde_egui::status_dot(ui, Style::WARN);
                    ui.add_space(Style::SP_XS);
                    mde_egui::muted_note(ui, format!("MOK posture undetermined \u{2014} {reason}"));
                });
            }
            MokEnrollment::ImportedAwaitingArm {
                firmware_prompt,
                arm_token: _,
                key_fingerprint,
            } => self.show_mok_arm(ui, firmware_prompt, key_fingerprint),
        }
    }

    fn show_mok_arm(&mut self, ui: &mut egui::Ui, firmware_prompt: &str, key_fingerprint: &str) {
        mde_egui::field(ui, "Key fingerprint", key_fingerprint, Style::TEXT);
        ui.add_space(Style::SP_XS);
        // The exact blue-screen firmware copy, verbatim (lock #6 — honest about
        // the manual firmware step no software can automate).
        ui.label(
            RichText::new(firmware_prompt)
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(
                Style::WARN,
                RichText::new("PENDING FIRMWARE ENROLLMENT")
                    .size(Style::SMALL)
                    .strong(),
            );
            mde_egui::muted_note(
                ui,
                "Reboot requires a separate host-state propose/confirm operation.",
            );
        });
    }

    fn show_firmware(&mut self, ui: &mut egui::Ui) {
        let Some(inv) = self.firmware.clone() else {
            mde_egui::muted_note(ui, "No firmware inventory yet.");
            return;
        };
        if let Some(reason) = &inv.skipped {
            mde_egui::muted_note(ui, format!("Firmware unavailable: {reason}"));
            return;
        }
        if let SurfaceAvailability::Unavailable { reason } | SurfaceAvailability::Stale { reason } =
            &inv.publication.availability
        {
            mde_egui::muted_note(ui, format!("Firmware unavailable: {reason}"));
            return;
        }
        if inv.devices.is_empty() {
            mde_egui::muted_note(ui, "fwupd reports no updatable devices.");
            return;
        }
        for dev in &inv.devices {
            ui.horizontal_wrapped(|ui| {
                let selectable = dev.update_available && dev.available_checksum.is_some();
                let selected = self.selected_fw.as_deref() == Some(dev.device_id.as_str());
                let tone = if dev.update_available {
                    Style::WARN
                } else {
                    Style::OK
                };
                mde_egui::status_dot(ui, tone);
                ui.add_space(Style::SP_XS);
                if selectable {
                    if ui
                        .selectable_label(selected, RichText::new(&dev.name).size(Style::SMALL))
                        .clicked()
                    {
                        self.selected_fw = Some(dev.device_id.clone());
                    }
                } else {
                    ui.label(
                        RichText::new(&dev.name)
                            .color(Style::TEXT)
                            .size(Style::SMALL),
                    );
                }
                ui.add_space(Style::SP_S);
                let ver = match &dev.available_version {
                    Some(av) if dev.update_available => {
                        format!("{} \u{2192} {} ({})", dev.current_version, av, dev.plugin)
                    }
                    _ => format!("{} ({})", dev.current_version, dev.plugin),
                };
                mde_egui::muted_note(ui, ver);
                if dev.update_available && dev.available_checksum.is_none() {
                    mde_egui::muted_note(ui, "no SHA-256; apply disabled");
                }
            });
        }

        // The last apply result, rendered as-is (§7).
        if let Some(res) = self.apply.clone() {
            ui.add_space(Style::SP_XS);
            let (tone, msg) = match &res.outcome {
                ApplyOutcome::Applied => (Style::OK, "applied \u{2014} re-verifying".to_string()),
                ApplyOutcome::Refused { reason } => {
                    (Style::WARN, format!("refused \u{2014} {reason}"))
                }
                ApplyOutcome::Gated { reason } => {
                    (Style::WARN, format!("integration-gated \u{2014} {reason}"))
                }
                ApplyOutcome::Failed { reason } => {
                    (Style::DANGER, format!("failed \u{2014} {reason}"))
                }
            };
            let msg = res
                .skipped
                .map_or(msg, |reason| format!("skipped \u{2014} {reason}"));
            ui.horizontal_wrapped(|ui| {
                mde_egui::status_dot(ui, tone);
                ui.add_space(Style::SP_XS);
                mde_egui::muted_note(ui, format!("{}: {msg}", res.device_id));
            });
        }

        // The typed-armed apply control.
        if let Some(device_id) = self.selected_fw.clone() {
            if let Some(device) = inv
                .devices
                .iter()
                .find(|device| device.device_id == device_id)
            {
                if let (Some(version), Some(checksum)) = (
                    device.available_version.as_deref(),
                    device.available_checksum.as_deref(),
                ) {
                    self.show_fw_apply_control(
                        ui,
                        &device.device_id,
                        inv.publication.published_at_ms,
                        version,
                        checksum,
                    );
                }
            }
        }
    }

    /// The SURFACE-5 typed-armed `fw-apply` control for the selected device:
    /// the arm-token input + an Apply button gated on the exact [`FW_ARM_TOKEN`]
    /// (lock #8 — a firmware apply is never automatic).
    fn show_fw_apply_control(
        &mut self,
        ui: &mut egui::Ui,
        device_id: &str,
        inventory_published_at_ms: u64,
        release_version: &str,
        release_checksum: &str,
    ) {
        ui.add_space(Style::SP_S);
        ui.vertical(|ui| {
            ui.label(
                RichText::new("Type to arm")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.fw_arm_input)
                    .hint_text(FW_ARM_TOKEN)
                    .desired_width(ui.available_width().min(Style::SP_XL * 6.0)),
            );
        });
        ui.add_space(Style::SP_XS);
        let armed = self.fw_arm_input.trim() == FW_ARM_TOKEN;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    armed,
                    egui::Button::new(RichText::new("Apply firmware").size(Style::BODY)),
                )
                .clicked()
            {
                let node = self.node.as_deref().unwrap_or_default();
                let unsigned = serde_json::to_string(&SurfaceFirmwareApplyRequest {
                    header: surface_action_header(node),
                    device_id: device_id.to_string(),
                    inventory_published_at_ms,
                    release_version: release_version.to_string(),
                    release_checksum: release_checksum.to_string(),
                    arm_token: Some(FW_ARM_TOKEN.to_string()),
                })
                .unwrap_or_default();
                let body = match crate::iac::authorize_root_mutation_body(
                    &unsigned,
                    "surface-firmware-apply",
                    node,
                    device_id.trim(),
                ) {
                    Ok(body) => body,
                    Err(error) => {
                        self.last_error =
                            Some(format!("Firmware apply authorization unavailable: {error}"));
                        return;
                    }
                };
                let topic = self.action_topic(fw_apply_action_topic);
                self.publish(
                    &topic,
                    &body,
                    "Firmware apply armed \u{2014} the node is applying it\u{2026}",
                );
                self.fw_arm_input.clear();
            }
            if !armed {
                mde_egui::muted_note(ui, "Type the exact token above to arm the apply.");
            }
        });
    }

    // ───────────────────────────── Test tab ─────────────────────────────

    fn show_test(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(RichText::new("Re-read board").size(Style::BODY))
                .clicked()
            {
                self.force_refresh();
            }
            mde_egui::muted_note(
                ui,
                "The node re-verifies every 30 s; this re-reads the latest.",
            );
        });
        ui.add_space(Style::SP_S);

        let Some(board) = self.board.clone() else {
            mde_egui::muted_note(ui, "No probe board published yet.");
            return;
        };
        if let Some(reason) = &board.skipped {
            mde_egui::muted_note(ui, format!("Verify skipped: {reason}"));
            return;
        }
        if board.rows.is_empty() {
            mde_egui::muted_note(ui, "No subsystems claimed by this model's profile.");
            return;
        }
        for row in &board.rows {
            ui.horizontal_wrapped(|ui| {
                mde_egui::status_dot(ui, probe_tone(row.state));
                ui.add_space(Style::SP_XS);
                ui.label(
                    RichText::new(shared_subsystem_label(row.subsystem))
                        .color(Style::TEXT)
                        .size(Style::SMALL),
                );
                ui.add_space(Style::SP_S);
                ui.colored_label(
                    probe_tone(row.state),
                    RichText::new(probe_word(row.state)).size(Style::SMALL),
                );
            });
            ui.horizontal_wrapped(|ui| {
                ui.add_space(Style::SP_M);
                mde_egui::muted_note(ui, &row.reason);
            });
            ui.add_space(Style::SP_XS);
        }
    }

    // ──────────────────────────── Config tab ────────────────────────────

    fn show_config(&mut self, ui: &mut egui::Ui) {
        // The applied per-model config knobs, read from the enable result (§7:
        // rendered from real Bus state — the daemon owns the per-model values;
        // they're applied by the governed local activation, with no raw
        // per-knob verb exposed to this card).
        ui.label(
            RichText::new("Applied Surface configuration")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL)
                .strong(),
        );
        ui.add_space(Style::SP_XS);
        let configs = self
            .enable
            .as_ref()
            .map(|e| e.activation.configs.clone())
            .unwrap_or_default();
        if configs.is_empty() {
            mde_egui::muted_note(ui, "No governed SAM profile result has been published yet.");
        } else {
            for cfg in &configs {
                ui.horizontal_wrapped(|ui| {
                    mde_egui::status_dot(ui, cfg.outcome.tone());
                    ui.add_space(Style::SP_XS);
                    ui.label(
                        RichText::new(cfg.key.label())
                            .color(Style::TEXT)
                            .size(Style::SMALL),
                    );
                    ui.add_space(Style::SP_S);
                    mde_egui::muted_note(ui, cfg.outcome.summary());
                });
            }
        }
        ui.add_space(Style::SP_XS);
        mde_egui::muted_note(
            ui,
            "Rotation and tablet-mode behavior follow live seat form-factor and IIO state in the DRM runner; activation does not write a rotation hint.",
        );

        ui.add_space(Style::SP_M);
        ui.separator();
        ui.add_space(Style::SP_S);

        // The SURFACE-7 DRM mode picker + fractional scale (in-process).
        ui.label(
            RichText::new("Display (DRM mode + scale)")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL)
                .strong(),
        );
        ui.add_space(Style::SP_XS);
        self.show_display(ui);
    }

    fn show_display(&mut self, ui: &mut egui::Ui) {
        #[cfg(feature = "drm")]
        if self.display.is_none() {
            self.display = mde_egui::runner_panel_info().map(DisplayController::runner);
        }
        let Some(ctrl) = self.display.as_mut() else {
            // No panel EDID readable (headless / farm / windowed): show the live
            // egui scale honestly rather than a fabricated picker.
            let ppp = ui.ctx().pixels_per_point();
            mde_egui::field(
                ui,
                "Active scale (egui)",
                &format!("{ppp:.2}\u{00D7}"),
                Style::TEXT,
            );
            mde_egui::muted_note(
                ui,
                "No panel EDID readable here \u{2014} the DRM mode picker is available when the shell owns the KMS seat (feature=drm on a real panel).",
            );
            return;
        };

        #[cfg(feature = "drm")]
        if let Some(panel) = mde_egui::runner_panel_info() {
            ctrl.update_panel(panel);
        }

        if let Some(ack) = ctrl.poll_pending_mode() {
            self.modeset_note = Some(match ack {
                Ok(mode) => format!(
                    "switched to {}×{} after DRM/GBM/EGL rebuild",
                    mode.width, mode.height
                ),
                Err(error) => error.to_string(),
            });
        }

        let native = *ctrl.native_mode();
        let active = *ctrl.active_mode();
        mde_egui::field(
            ui,
            "Native",
            &format!(
                "{}\u{00D7}{} @ {:.0} Hz",
                native.width,
                native.height,
                native.refresh_hz()
            ),
            Style::TEXT,
        );
        mde_egui::field(
            ui,
            "Active",
            &format!(
                "{}\u{00D7}{} @ {:.0} Hz",
                active.width,
                active.height,
                active.refresh_hz()
            ),
            if active == native {
                Style::TEXT
            } else {
                Style::ACCENT
            },
        );
        ui.add_space(Style::SP_XS);

        // The mode picker (native ↔ HD, lock 12). HD is offered only when the
        // connector actually advertises 1920×1080 (never fabricated).
        ui.horizontal_wrapped(|ui| {
            let modes: Vec<mde_egui::PanelMode> = ctrl.modes().to_vec();
            for mode in modes {
                let selected = mode == active;
                let label = match mode.class() {
                    ModeClass::Native => "Native".to_string(),
                    ModeClass::Hd => "HD 1080p".to_string(),
                    ModeClass::Other => format!("{}\u{00D7}{}", mode.width, mode.height),
                };
                if ui
                    .selectable_label(selected, RichText::new(label).size(Style::SMALL))
                    .clicked()
                {
                    match ctrl.request_mode(&mode) {
                        Ok(ModesetDispatch::Applied) => {
                            self.modeset_note =
                                Some(format!("switched to {}\u{00D7}{}", mode.width, mode.height));
                        }
                        Ok(ModesetDispatch::Queued { request_id }) => {
                            self.modeset_note = Some(format!(
                                "rebuilding DRM scanout for {}×{} (request {request_id})…",
                                mode.width, mode.height
                            ));
                        }
                        // Honest gated state — the headless seam refuses; a real
                        // KMS seat applies (§7 — never faked).
                        Err(e) => self.modeset_note = Some(e.to_string()),
                    }
                }
                ui.add_space(Style::SP_XS);
            }
        });
        if let Some(note) = self.modeset_note.clone() {
            mde_egui::muted_note(ui, &note);
        }
        ui.add_space(Style::SP_S);
        show_scale_control(ui, ctrl);
    }

    /// Build a per-node action/state topic, given the discovered node id. Only
    /// called from the render path, which is reached only when `node` is `Some`
    /// (the summary gated us in); defends with an empty id otherwise.
    fn action_topic(&self, f: fn(&str) -> String) -> String {
        f(self.node.as_deref().unwrap_or(""))
    }
}

/// The SURFACE-7 fractional-scale control (lock 11). Unlike the KMS mode
/// picker, the scale IS applied live in-process (egui `pixels_per_point`), so
/// the slider is a real, immediate control on any seat. A free fn (not a method)
/// so it borrows only the already-`&mut`-borrowed [`DisplayController`], not all
/// of the card.
fn show_scale_control(ui: &mut egui::Ui, ctrl: &mut DisplayController) {
    let mut scale = ctrl.effective_scale();
    let computed = ctrl.computed_scale();
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new("Scale")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);
        if ui
            .add(
                egui::Slider::new(
                    &mut scale,
                    mde_egui::display::MIN_SCALE..=mde_egui::display::MAX_SCALE,
                )
                .step_by(0.25)
                .fixed_decimals(2),
            )
            .changed()
        {
            ctrl.set_scale_override(Some(scale));
            ui.ctx().set_pixels_per_point(ctrl.effective_scale());
        }
    });
    ui.horizontal_wrapped(|ui| {
        mde_egui::muted_note(ui, format!("panel-computed {computed:.2}\u{00D7}"));
        ui.add_space(Style::SP_S);
        if ctrl.scale_override().is_some() && ui.small_button("reset").clicked() {
            ctrl.set_scale_override(None);
            ui.ctx().set_pixels_per_point(ctrl.effective_scale());
        }
    });
}

/// Discover this node's `<node>` id from the Bus: the summary topic
/// `state/hardware/surface/<node>` (no further path segment) a Surface node
/// publishes. `None` when no Surface summary exists (the model gate is closed).
fn discover_node(persist: &Persist, expected_node: &str) -> Option<String> {
    if expected_node.trim().is_empty() {
        return None;
    }
    let expected_topic = summary_topic(expected_node);
    let topics = persist.list_topics().ok()?;
    topics
        .iter()
        .any(|topic| topic == &expected_topic)
        .then(|| expected_node.to_string())
}

/// Read the latest message on `topic`, advancing `cursor`, decoding into `slot`.
/// A message that fails to decode is skipped (the last good value is kept).
fn read_latest<T: for<'de> Deserialize<'de>>(
    persist: &Persist,
    topic: &str,
    cursor: &mut Option<String>,
    slot: &mut Option<T>,
) {
    let Ok(msgs) = persist.list_since(topic, cursor.as_deref()) else {
        return;
    };
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        if let Some(body) = msg.body.as_deref() {
            if let Ok(decoded) = serde_json::from_str::<T>(body) {
                *slot = Some(decoded);
            }
        }
    }
}

fn read_latest_surface_summary(
    persist: &Persist,
    topic: &str,
    expected_node: &str,
    cursor: &mut Option<String>,
    slot: &mut Option<SurfaceFleetSummary>,
) {
    let now_ms = wall_clock_ms();
    read_latest_with(persist, topic, cursor, slot, |body| {
        decode_surface_summary_at(body, expected_node, now_ms)
    });
}

fn read_latest_surface_board(
    persist: &Persist,
    topic: &str,
    expected_node: &str,
    cursor: &mut Option<String>,
    slot: &mut Option<SurfaceVerifyBoard>,
) {
    let now_ms = wall_clock_ms();
    read_latest_with(persist, topic, cursor, slot, |body| {
        decode_surface_board_at(body, expected_node, now_ms)
    });
}

fn read_latest_surface_firmware(
    persist: &Persist,
    topic: &str,
    expected_node: &str,
    cursor: &mut Option<String>,
    slot: &mut Option<SurfaceFirmwareInventory>,
) {
    let now_ms = wall_clock_ms();
    read_latest_with(persist, topic, cursor, slot, |body| {
        let mut value = SurfaceFirmwareInventory::from_json(body).ok()?;
        admit_publication_freshness(&mut value.publication, expected_node, now_ms)?;
        Some(value)
    });
}

fn wall_clock_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .unwrap_or_default()
}

fn admit_publication_freshness(
    publication: &mut mackes_mesh_types::surface_hardware::SurfacePublication,
    expected_node: &str,
    now_ms: u64,
) -> Option<()> {
    if publication.node != expected_node {
        return None;
    }
    if matches!(publication.availability, SurfaceAvailability::Fresh) {
        if publication.published_at_ms > now_ms.saturating_add(MAX_SURFACE_STATE_FUTURE_SKEW_MS) {
            return None;
        }
        if now_ms.saturating_sub(publication.published_at_ms) > MAX_SURFACE_STATE_AGE_MS {
            publication.availability = SurfaceAvailability::Stale {
                reason: "no admitted Surface verification publication for 90 seconds".into(),
            };
        }
    }
    Some(())
}

fn decode_surface_summary_at(
    body: &[u8],
    expected_node: &str,
    now_ms: u64,
) -> Option<SurfaceFleetSummary> {
    let mut value = SurfaceFleetSummary::from_json(body).ok()?;
    admit_publication_freshness(&mut value.publication, expected_node, now_ms)?;
    Some(value)
}

fn decode_surface_board_at(
    body: &[u8],
    expected_node: &str,
    now_ms: u64,
) -> Option<SurfaceVerifyBoard> {
    let mut value = SurfaceVerifyBoard::from_json(body).ok()?;
    admit_publication_freshness(&mut value.publication, expected_node, now_ms)?;
    Some(value)
}

/// Read an untrusted Bus lane through its bounded contract decoder. The cursor
/// advances over rejected messages so a hostile record cannot pin polling;
/// the last admitted value remains visible until a valid replacement arrives.
fn read_latest_with<T>(
    persist: &Persist,
    topic: &str,
    cursor: &mut Option<String>,
    slot: &mut Option<T>,
    decode: impl Fn(&[u8]) -> Option<T>,
) {
    let Ok(msgs) = persist.list_since(topic, cursor.as_deref()) else {
        return;
    };
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        if let Some(body) = msg.body.as_deref() {
            if let Some(decoded) = decode(body.as_bytes()) {
                *slot = Some(decoded);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::surface_hardware::{
        SurfaceModelIdentity, SurfaceObservationSource, SurfaceProGeneration, SurfaceProbeVerdict,
        SurfacePublication, MAX_SURFACE_WIRE_BYTES,
    };
    use mde_bus::persist::Persist;
    use mde_egui::egui::{pos2, vec2, Rect};
    use mde_egui::{Density, PanelMode, StyleColorScheme};

    const ACTION_NOW_MS: u64 = 1_800_000_000_000;

    fn test_action_header(request_id: &str) -> SurfaceActionHeader {
        SurfaceActionHeader {
            schema_version: SURFACE_HARDWARE_SCHEMA_VERSION,
            node: "this-node".into(),
            request_id: request_id.into(),
            issued_at_ms: ACTION_NOW_MS,
            armed_token: None,
        }
    }

    fn test_publication() -> SurfacePublication {
        SurfacePublication {
            schema_version: SURFACE_HARDWARE_SCHEMA_VERSION,
            node: "this-node".into(),
            model: SurfaceModelIdentity {
                product: "Surface Pro 6".into(),
                generation: SurfaceProGeneration::Pro6,
            },
            source: SurfaceObservationSource::Kernel,
            published_at_ms: ACTION_NOW_MS,
            availability: SurfaceAvailability::Fresh,
        }
    }

    /// A recognised-Surface fixture state: a summary (the model gate open), a
    /// probe board, an enable result mid-MOK, a firmware inventory, and an
    /// injected display controller — everything the three tabs render.
    fn fixture() -> SurfaceCardState {
        let native = PanelMode::new(2880, 1920, 60, true);
        let panel = PanelInfo::new(native, (260, 173), &[PanelMode::new(1920, 1080, 60, false)]);
        SurfaceCardState {
            bus_root: None,
            node: Some("this-node".to_string()),
            summary: Some(SurfaceFleetSummary {
                publication: test_publication(),
                enablement_pct: 75,
                red_count: 1,
                red_subsystems: vec![SurfaceSubsystem::Cameras],
            }),
            board: Some(SurfaceVerifyBoard {
                publication: test_publication(),
                skipped: None,
                rows: vec![
                    SurfaceProbeVerdict {
                        subsystem: SurfaceSubsystem::Touch,
                        state: SurfaceProbeState::Ok,
                        reason: "touchscreen enumerated (IPTS)".to_string(),
                    },
                    SurfaceProbeVerdict {
                        subsystem: SurfaceSubsystem::Pen,
                        state: SurfaceProbeState::NeedsGesture,
                        reason: "press the pen to the screen".to_string(),
                    },
                    SurfaceProbeVerdict {
                        subsystem: SurfaceSubsystem::Cameras,
                        state: SurfaceProbeState::Failed,
                        reason: "no V4L2 capture device".to_string(),
                    },
                ],
            }),
            enable: Some(EnableResult {
                model: "Surface Pro 6".to_string(),
                skipped: None,
                activation: ActivationResult {
                    units: vec![UnitResult {
                        unit: "iptsd@.service".to_string(),
                        outcome: StepOutcome::AlreadyActive,
                    }],
                    configs: vec![ConfigResult {
                        key: ConfigKey::SamPerfProfile,
                        subsystem: Subsystem::Sam,
                        outcome: StepOutcome::Applied,
                    }],
                },
                mok: MokEnrollment::ImportedAwaitingArm {
                    firmware_prompt: "After the reboot the firmware shows a blue screen..."
                        .to_string(),
                    arm_token: "REBOOT-TO-ENROLL-MOK".to_string(),
                    key_fingerprint: "SHA1:ab:cd:ef".to_string(),
                },
            }),
            firmware: Some(SurfaceFirmwareInventory {
                publication: SurfacePublication {
                    source: SurfaceObservationSource::Fwupd,
                    ..test_publication()
                },
                skipped: None,
                devices: vec![mackes_mesh_types::surface_hardware::SurfaceFirmwareDevice {
                    device_id: "dev-uefi".to_string(),
                    name: "System Firmware".to_string(),
                    plugin: "uefi_capsule".to_string(),
                    current_version: "1.2.9".to_string(),
                    available_version: Some("1.2.10".to_string()),
                    available_checksum: Some("a".repeat(64)),
                    update_available: true,
                }],
            }),
            apply: None,
            display: Some(DisplayController::headless(panel)),
            ..SurfaceCardState::default_no_probe()
        }
    }

    impl SurfaceCardState {
        /// A default that never touches sysfs for the panel (tests inject one).
        fn default_no_probe() -> Self {
            Self {
                display: None,
                ..Self::bare()
            }
        }
        fn bare() -> Self {
            Self {
                bus_root: None,
                node: None,
                summary: None,
                board: None,
                enable: None,
                firmware: None,
                apply: None,
                tab: Tab::default(),
                fw_arm_input: String::new(),
                selected_fw: None,
                action_note: None,
                last_error: None,
                display: None,
                modeset_note: None,
                cur_summary: None,
                cur_board: None,
                cur_enable: None,
                cur_firmware: None,
                cur_apply: None,
                last_poll: None,
            }
        }
    }

    /// Drive one headless 960×720 frame with the card on `tab` and tessellate it
    /// on the CPU — the same `Context::run` → `tessellate` path the DRM runner
    /// drives minus the GPU. Returns whether it produced any draw primitives.
    fn renders(state: &mut SurfaceCardState, tab: Tab) -> bool {
        state.tab = tab;
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 720.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| state.show(ui));
        });
        !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty()
    }

    fn render_shapes_at(
        state: &mut SurfaceCardState,
        tab: Tab,
        width: f32,
        scheme: StyleColorScheme,
        density: Density,
        zoom: f32,
    ) -> (Vec<egui::epaint::ClippedShape>, Rect) {
        state.tab = tab;
        let ctx = egui::Context::default();
        Style::install_color_scheme_with_density(&ctx, scheme, density);
        ctx.set_zoom_factor(zoom);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 1600.0))),
            ..Default::default()
        };
        // egui applies a requested zoom at the next begin-pass. Prime one pass
        // so the measured layout uses both the intended viewport and text scale.
        let _ = ctx.run(input.clone(), |_| {});
        let mut content_rect = Rect::NOTHING;
        let output = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.show(ui);
                content_rect = ui.min_rect();
            });
        });
        (output.shapes, content_rect)
    }

    #[test]
    fn card_is_gated_off_without_a_surface_summary() {
        // No summary ⇒ not a Surface ⇒ the gate is closed and the card draws
        // nothing (design lock #3/#7).
        let mut s = SurfaceCardState::bare();
        assert!(!s.is_surface(), "no summary ⇒ not gated in");
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| s.show(ui));
        });
        // The CentralPanel frame itself paints; the card adds nothing — assert
        // show() early-returns by checking is_surface stays false + no panic.
        let _ = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(s.summary.is_none());
    }

    #[test]
    fn each_tab_renders_from_fixture_bus_state() {
        let mut s = fixture();
        assert!(s.is_surface(), "the summary opens the model gate");
        assert!(
            renders(&mut s, Tab::Install),
            "Install tab produced no primitives"
        );
        assert!(
            renders(&mut s, Tab::Test),
            "Test tab produced no primitives"
        );
        assert!(
            renders(&mut s, Tab::Config),
            "Config tab produced no primitives"
        );
    }

    #[test]
    fn surface_card_reflows_at_narrow_touch_and_large_text_in_dark_and_light() {
        for scheme in [StyleColorScheme::Dark, StyleColorScheme::Light] {
            for (width, density, zoom) in [
                (320.0, Density::Touch, 1.35),
                (480.0, Density::Touch, 1.15),
                (960.0, Density::Mouse, 1.0),
            ] {
                for tab in Tab::ALL {
                    let mut state = fixture();
                    let (shapes, content_rect) =
                        render_shapes_at(&mut state, tab, width, scheme, density, zoom);
                    assert!(
                        !shapes.is_empty(),
                        "{tab:?} produced no shapes at {width}px/{scheme:?}/{density:?}/{zoom}x"
                    );
                    assert!(
                        content_rect.is_finite()
                            && content_rect.min.x >= -1.0
                            && content_rect.max.x <= width + 1.0,
                        "{tab:?} laid out {content_rect:?} outside the horizontal viewport at {width}px/{scheme:?}/{density:?}/{zoom}x"
                    );
                }
            }
        }
    }

    #[test]
    fn firmware_request_serialises_to_the_worker_wire_shape() {
        let firmware = SurfaceFirmwareApplyRequest {
            header: test_action_header("firmware-apply"),
            device_id: "dev-uefi".to_string(),
            inventory_published_at_ms: ACTION_NOW_MS,
            release_version: "1.2.10".to_string(),
            release_checksum: "a".repeat(64),
            arm_token: Some(FW_ARM_TOKEN.to_string()),
        };
        let firmware_json = serde_json::to_vec(&firmware).unwrap();
        assert_eq!(
            SurfaceFirmwareApplyRequest::from_json_at(&firmware_json, "this-node", ACTION_NOW_MS),
            Ok(firmware)
        );
    }

    #[test]
    fn shared_surface_state_decodes_the_worker_wire_bodies() {
        let fixture = fixture();
        let board_body = serde_json::to_vec(fixture.board.as_ref().unwrap()).unwrap();
        let board = decode_surface_board_at(&board_body, "this-node", ACTION_NOW_MS)
            .expect("board decodes");
        assert_eq!(board.rows[0].subsystem, SurfaceSubsystem::Touch);
        assert_eq!(board.rows[1].state, SurfaceProbeState::NeedsGesture);

        let mok: EnableResult = serde_json::from_str(
            r#"{"model":"Surface Pro 6","skipped":null,
                "activation":{"units":[{"unit":"iptsd@.service","outcome":"AlreadyActive"}],
                  "configs":[{"key":"SamPerfProfile","subsystem":"Sam","outcome":"Applied"}]},
                "mok":{"Enrolled":{"modules_loaded":true}}}"#,
        )
        .expect("enable decodes");
        assert!(matches!(
            mok.mok,
            MokEnrollment::Enrolled {
                modules_loaded: true
            }
        ));
        assert!(matches!(
            mok.activation.configs[0].outcome,
            StepOutcome::Applied
        ));

        let summary_body = serde_json::to_vec(fixture.summary.as_ref().unwrap()).unwrap();
        let summary = decode_surface_summary_at(&summary_body, "this-node", ACTION_NOW_MS)
            .expect("summary decodes");
        assert_eq!(summary.enablement_pct, 75);
    }

    #[test]
    fn shared_surface_state_rejects_hostile_wire_bodies() {
        let valid_board = serde_json::to_vec(fixture().board.as_ref().unwrap()).unwrap();
        let valid_summary = serde_json::to_vec(fixture().summary.as_ref().unwrap()).unwrap();
        assert!(decode_surface_board_at(&valid_board, "this-node", ACTION_NOW_MS).is_some());
        assert!(decode_surface_summary_at(&valid_summary, "this-node", ACTION_NOW_MS).is_some());

        let unknown_board = String::from_utf8(valid_board.clone()).unwrap().replacen(
            "\"skipped\":null",
            "\"skipped\":null,\"surprise\":true",
            1,
        );
        assert!(
            decode_surface_board_at(unknown_board.as_bytes(), "this-node", ACTION_NOW_MS).is_none()
        );

        let duplicate_summary = String::from_utf8(valid_summary.clone()).unwrap().replacen(
            "\"enablement_pct\":75",
            "\"enablement_pct\":75,\"enablement_pct\":75",
            1,
        );
        assert!(decode_surface_summary_at(
            duplicate_summary.as_bytes(),
            "this-node",
            ACTION_NOW_MS
        )
        .is_none());

        let oversized = vec![b' '; MAX_SURFACE_WIRE_BYTES + 1];
        assert!(decode_surface_board_at(&oversized, "this-node", ACTION_NOW_MS).is_none());
        assert!(decode_surface_summary_at(&oversized, "this-node", ACTION_NOW_MS).is_none());

        assert!(decode_surface_board_at(&valid_board, "foreign-node", ACTION_NOW_MS).is_none());
        assert!(decode_surface_summary_at(&valid_summary, "foreign-node", ACTION_NOW_MS).is_none());
    }

    #[test]
    fn fresh_surface_state_ages_to_stale_and_future_state_is_refused() {
        let summary_body = serde_json::to_vec(fixture().summary.as_ref().unwrap()).unwrap();
        let stale = decode_surface_summary_at(
            &summary_body,
            "this-node",
            ACTION_NOW_MS + MAX_SURFACE_STATE_AGE_MS + 1,
        )
        .expect("old state is retained with an honest stale label");
        assert!(matches!(
            stale.publication.availability,
            SurfaceAvailability::Stale { .. }
        ));
        assert!(
            decode_surface_summary_at(
                &summary_body,
                "this-node",
                ACTION_NOW_MS - MAX_SURFACE_STATE_FUTURE_SKEW_MS - 1,
            )
            .is_none(),
            "implausibly future state is refused"
        );
    }

    #[test]
    fn discover_node_requires_the_exact_local_summary_lane() {
        let dir = std::env::temp_dir().join(format!(
            "mde-surfcard-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let persist = Persist::open(dir.clone()).expect("open bus");
        persist
            .write(
                "state/hardware/surface/anvil/probes",
                Priority::Default,
                None,
                Some("{}"),
            )
            .expect("write probes");
        persist
            .write(
                "state/hardware/surface/remote-surface",
                Priority::Default,
                None,
                Some("{}"),
            )
            .expect("write remote summary");
        assert_eq!(discover_node(&persist, "anvil"), None);
        persist
            .write(
                "state/hardware/surface/anvil",
                Priority::Default,
                None,
                Some("{}"),
            )
            .expect("write local summary");
        assert_eq!(discover_node(&persist, "anvil").as_deref(), Some("anvil"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refresh_clears_retained_surface_when_the_local_topic_is_absent() {
        let dir = std::env::temp_dir().join(format!(
            "mde-surfcard-clear-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        let persist = Persist::open(dir.clone()).expect("open replacement bus");
        persist
            .write(
                "state/hardware/surface/remote-only",
                Priority::Default,
                None,
                Some("{}"),
            )
            .expect("write remote summary");

        let mut state = fixture();
        assert!(state.summary.is_some(), "fixture begins as a Surface");
        state.bus_root = Some(dir.clone());
        state.refresh();
        assert!(state.node.is_none());
        assert!(state.summary.is_none());
        assert!(state.board.is_none());
        assert!(state.firmware.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
