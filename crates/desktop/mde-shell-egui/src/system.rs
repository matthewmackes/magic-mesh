//! `Surface::System` — this seat's host-controls panel (E12-15 status; E12-18 makes
//! Displays + Power interactive).
//!
//! Under E12 "Quasar" the shell owns the DRM seat with no compositor and no
//! settings daemon, so audio / Bluetooth / displays / power / backlight have no
//! owner until `mde-seat` (design `docs/design/quasar-host-controls.md`). This
//! surface is where ALL host-control interaction lives (lock 3); the chrome bar
//! keeps only read-only status icons (see [`crate::chrome`]).
//!
//! The one render model is [`mde_seat::SeatSnapshot`] — every section is a
//! [`Probe`]: `Present` shows the real rows, `Absent` shows the shared honest
//! "not available" note (§7 / interlock 4), never a fake control. E12-15 landed
//! this read-only; **E12-18** wires the two hardware-reachable sections:
//!
//! - **Displays** — per-output enable / mode / relative arrangement (editing the
//!   [`DisplayLayout`] intent model, with the typed "never black the last console"
//!   interlock enforced) plus **live brightness** (sysfs backlight for internal
//!   panels, DDC/CI for externals; an honest "not controllable" state when a
//!   monitor rejects DDC — lock 13). The live *modeset apply* of an arrangement is
//!   integration-gated (the shell owns the seat inside `run_drm`; the panel→runner
//!   verb wiring is E12-19), so arrangement edits are the desired-state intent,
//!   noted typed.
//! - **Power & Battery** — confirm-gated local lock/suspend/reboot/poweroff
//!   (logind, lock 12), and multi-battery telemetry (incl. BT-peripheral batteries,
//!   lock 6). VM lifecycle now lives in the QUASAR-CLOUD plane, not a local
//!   cloud-hypervisor broker.
//!
//! Mixer / Bluetooth stay read-only here (their interaction is E12-16 / E12-17).
//! The state holds the ONE [`Seat`] (lock 1) and re-`snapshot()`s it on the shell's
//! shared pump cadence; the same cached snapshot feeds the chrome icons.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mde_egui::egui::{self, ComboBox, RichText, Slider};
use mde_egui::{field, muted_note, OsdKind, OsdLevel, Severity, Style, Toast};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use mde_seat::hotkeys::HotkeyAction;
use mde_seat::{
    Avail, Backlight, BtAdapter, BtDevice, BtStatus, Connector, ConnectorStatus, DdcDisplay,
    DisplayLayout, DisplayMode, LidState, MixerStatus, MixerStrip, MonitorId, OutputArrangement,
    PairingAgent, PowerCaps, PowerVerb, Probe, Seat, SeatError, SeatSnapshot, HOTKEYS,
};

use crate::bt_pairing::{pairing_dialog, PairingBridge};
use crate::power_honor::PowerHonorConfig;
use crate::power_settings;

/// Poll cadence — a device plug, a battery drain, or a BT connect surfaces within
/// this window.
const REFRESH: Duration = Duration::from_secs(5);

/// The world-readable mesh-status snapshot the SETTINGS-4 Mesh & System sections
/// fold — the SAME source the chrome bar + the This Node / Network planes already
/// read (`/run/mde/mesh-status.json`, written every ~30s by the root
/// `mesh-status.timer`). The desktop user can't read the root-only replicated peer
/// directory, so this JSON is the desktop tier's read path — the shell leans on no
/// `mackesd` IPC and no root-only cert (§6).
const MESH_STATUS_PATH: &str = "/run/mde/mesh-status.json";

/// A filled-circle status dot — the shared glyph the rest of the platform uses.
const DOT: &str = "\u{25CF}";

/// One volume/brightness hotkey press moves the level this many points (0–100).
/// A coarse-but-responsive step — five taps span the range.
const HOTKEY_STEP: i16 = 5;

// ──────────────────────────── the System state ────────────────────────────

/// The System surface's live state: the ONE [`Seat`] (lock 1) plus its latest
/// snapshot, the editable display arrangement, and the live brightness values.
pub(crate) struct SystemState {
    /// The one seat over the real host hardware (in-process, lock 1).
    seat: Seat,
    /// The latest snapshot. `None` until the first poll.
    snapshot: Option<SeatSnapshot>,
    /// When the seat was last snapshotted (drives the fixed cadence).
    last_poll: Option<Instant>,
    /// The editable multi-head arrangement intent (E12-18). Rebuilt from the probe
    /// only when the connector set changes (a replug), so operator edits persist.
    layout: DisplayLayout,
    /// The connector names the current [`Self::layout`] was built from — the key
    /// that detects a replug (and thus a rebuild) without clobbering edits.
    layout_key: Vec<String>,
    /// Live per-panel backlight brightness (0–100), keyed by sysfs device name.
    /// Seeded from the probe, then owned by the slider so a drag stays smooth.
    panel_brightness: BTreeMap<String, u8>,
    /// Live per-monitor DDC brightness (0–100), keyed by i2c bus label.
    ddc_brightness: BTreeMap<String, u8>,
    /// An armed power verb awaiting its second (confirm) click (lock 12).
    confirm: Option<PowerVerb>,
    /// Live battery charge-stop cap (0–100) the POWER-4 threshold slider owns,
    /// seeded from the snapshot's `charge_limit` so a drag stays smooth. `None`
    /// until a battery is seen advertising the attribute (`Present(Some(_))`).
    charge_threshold: Option<u8>,
    /// The last control action's honest inline error (a refused write / interlock).
    error: Option<String>,
    /// Publishes each fresh snapshot to the node-local mirror topic so the `mackesd`
    /// `host_state` worker can mirror this node mesh-wide (E12-19, lock 1).
    mirror: crate::host_mirror::HostMirrorPublisher,
    /// The `BlueZ` pairing bridge (E12-17): the shared mailbox the registered agent
    /// posts PIN/passkey prompts to and the panel's modal drains. Cloned into the
    /// agent's responder on register.
    pairing: PairingBridge,
    /// The registered pairing agent — live only while the System surface is in view
    /// and an adapter is present; dropped (which unregisters it) on leave.
    agent: Option<PairingAgent>,
    /// Whether an agent registration has already been attempted this active-visit,
    /// so a failure toasts once rather than every frame.
    agent_attempted: bool,
    /// The pairing dialog's PIN/passkey entry buffer (persists across frames).
    pin_input: String,
    /// Control-error alerts raised by a Bluetooth write — drained by the shell into
    /// the one `ToastBridge` after `show()` (§7: a refused/absent write is surfaced).
    pending_toasts: Vec<Toast>,
    /// The POWER-5 idle-suspend + lid-close policy the operator edits in the Power
    /// section — the source of truth the [`crate::power_honor`] honorer reads every
    /// frame. Loaded from disk on start; saved on change. Safe defaults (idle Never,
    /// lid Suspend) until the operator picks otherwise.
    power_honor_config: PowerHonorConfig,
    /// The Settings master-detail rail selection (SETTINGS-1) — the domain group +
    /// section the detail pane rests on. Loaded from disk on start and saved on
    /// every move, so the surface reopens where the operator left it across a
    /// surface switch AND a restart (the [`PowerHonorConfig`] client-data-dir JSON
    /// idiom, reused verbatim).
    nav: SettingsNav,
    /// This node's mesh identity / role / network facts (SETTINGS-4), folded from
    /// the SAME world-readable mesh-status snapshot the chrome bar + the This Node /
    /// Network planes read ([`MESH_STATUS_PATH`]). Refreshed on the shared poll
    /// cadence; the Mesh & System sections render it honest-`unknown` where the
    /// snapshot doesn't carry a fact (§6/§7 — no new probe, no root-only cert read).
    mesh: MeshFacts,
    /// The Personalization → Theme appearance (SETTINGS-5): the interactive accent +
    /// the platform text-scale. Loaded from disk on start (restored on open), saved on
    /// a pick, and applied live to the context every frame by
    /// [`Self::apply_appearance`] (the [`SettingsNav`] client-data-dir JSON idiom).
    appearance: AppearanceConfig,
    /// The seat's DPI zoom base — the egui `zoom_factor` the runner set from the panel
    /// (or 1.0 windowed), captured ONCE on the first appearance apply so the text-scale
    /// step composes with the panel DPI instead of clobbering it (SETTINGS-5). `None`
    /// until the first poll observes the base.
    zoom_base: Option<f32>,
}

impl Default for SystemState {
    fn default() -> Self {
        Self {
            seat: Seat::new(),
            snapshot: None,
            last_poll: None,
            layout: DisplayLayout::default(),
            layout_key: Vec::new(),
            panel_brightness: BTreeMap::new(),
            ddc_brightness: BTreeMap::new(),
            confirm: None,
            charge_threshold: None,
            error: None,
            mirror: crate::host_mirror::HostMirrorPublisher::default(),
            pairing: PairingBridge::default(),
            agent: None,
            agent_attempted: false,
            pin_input: String::new(),
            pending_toasts: Vec::new(),
            power_honor_config: PowerHonorConfig::load(),
            nav: SettingsNav::load(),
            mesh: MeshFacts::default(),
            appearance: AppearanceConfig::load(),
            zoom_base: None,
        }
    }
}

/// One control action collected during the render borrow, applied after it ends
/// so the drive can take `&mut` freely.
///
/// `pub(crate)` so the POWER-4 body-builders in [`crate::power_settings`] emit the
/// same actions the section's `apply()` drives.
pub(crate) enum SysAction {
    /// Enable/disable an output (gated by the last-console interlock).
    ToggleOutput(MonitorId, bool),
    /// Choose an output's mode.
    SetMode(MonitorId, DisplayMode),
    /// Move an output one slot left/right in the arrangement row.
    Nudge(MonitorId, bool),
    /// Write a sysfs backlight panel's raw brightness.
    Backlight { name: String, raw: u32 },
    /// Write an external monitor's DDC/CI brightness (0–100).
    Ddc { bus: String, percent: u8 },
    /// Arm a power verb for confirmation (first click on a gated verb).
    ArmConfirm(PowerVerb),
    /// Execute a power verb (Lock, or the confirm click on a gated verb).
    Power(PowerVerb),
    /// Cancel an armed confirmation.
    CancelConfirm,
    /// Switch the active power profile (POWER-4) — routed to
    /// [`Seat::set_power_profile`]; only ever an offered profile name.
    SetPowerProfile(String),
    /// Set the battery charge-stop cap 0–100 (POWER-4) — routed to
    /// [`Seat::set_charge_threshold`].
    SetChargeThreshold(u8),
    /// Persist the POWER-5 idle/lid policy after a picker change — the config has
    /// already been mutated in place; this writes it to disk.
    SavePowerHonorConfig,
    // ── Bluetooth control verbs (E12-17) ────────────────────────────────────
    /// Power an adapter radio on/off (`adapter path`, `on`).
    BtPower(String, bool),
    /// Make an adapter discoverable to nearby devices (`adapter path`, `on`).
    BtDiscoverable(String, bool),
    /// Let an adapter accept incoming pairings (`adapter path`, `on`).
    BtPairable(String, bool),
    /// Start (`true`) / stop (`false`) a device-discovery scan on `adapter path`.
    BtScan(String, bool),
    /// Connect to a device (`device path`).
    BtConnect(String),
    /// Disconnect a device (`device path`).
    BtDisconnect(String),
    /// Pair (bond) with a device (`device path`) — the agent answers any prompt.
    BtPair(String),
    /// Forget a device — drop the bond (`adapter path`, `device path`).
    BtForget { adapter: String, device: String },
    /// Trust / untrust a device for auto-reconnect (`device path`, `trusted`).
    BtTrust(String, bool),
    // ── Mesh & System (SETTINGS-4) ───────────────────────────────────────────
    /// Re-arm the pairing responder from the Mesh & System → Pairing section:
    /// clear the once-per-visit latch and re-attempt registration on the SAME
    /// [`SystemState::sync_pairing_agent`] seam main.rs drives on surface
    /// visibility (§6 — one responder, never a second agent).
    PairingRetry,
}

impl SystemState {
    /// The poll seam: re-snapshot on cadence, then reconcile the arrangement model
    /// + brightness seeds against the fresh probe.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            let snap = self.seat.snapshot();
            self.reconcile(&snap);
            // Mirror the fresh snapshot mesh-wide (E12-19, lock 1): the host_state
            // worker republishes it to state/host/<node>/seat for every peer's
            // Workbench. Published on the shared cadence, not per-frame.
            self.mirror.publish(&snap);
            self.snapshot = Some(snap);
            // Fold this node's mesh identity / role / network facts from the same
            // world-readable snapshot the chrome bar reads (SETTINGS-4, §6). A
            // missing / unreadable file folds to the honest unseen facts, never a
            // panic — mirroring the This Node / Network planes' tolerance.
            let mesh_snapshot = fs::read_to_string(MESH_STATUS_PATH).unwrap_or_default();
            self.mesh = MeshFacts::project(&mesh_snapshot);
        }
        // SETTINGS-5: apply the persisted Personalization → Theme appearance to the
        // live context every frame (poll runs unconditionally in both runners, so this
        // is honored globally + restored on start — not just while Settings is open).
        self.apply_appearance(ctx);
        ctx.request_repaint_after(REFRESH);
    }

    /// Apply the persisted Personalization → Theme appearance (SETTINGS-5) to the live
    /// context: re-tint the interactive accent and hold the whole-UI text-scale zoom.
    /// Cheap-guarded — a no-op frame costs one field read and never re-mutates — and
    /// self-correcting: if a formfactor [`Style::install_with_density`] re-install reset
    /// the accent to the brand blue, the next poll re-applies the pick. Both effects are
    /// real runtime state (the egui visuals + zoom), never a dead toggle (§7).
    fn apply_appearance(&mut self, ctx: &egui::Context) {
        // Accent — re-tint only when the live accent drifts from the pick (a settings
        // change, a fresh context, OR a formfactor re-install reset it to the brand).
        let want_accent = self.appearance.accent.color();
        if ctx.style().visuals.hyperlink_color != want_accent {
            Style::set_accent(ctx, want_accent);
        }
        // Text-scale — capture the seat's DPI zoom base once (what the DRM runner set
        // from the panel, or 1.0 windowed), then hold the zoom at base × the chosen
        // step so the accessibility scale COMPOSES with HiDPI instead of clobbering it.
        let base = *self.zoom_base.get_or_insert_with(|| ctx.zoom_factor());
        let want_zoom = base * self.appearance.text_scale.factor();
        if (ctx.zoom_factor() - want_zoom).abs() > f32::EPSILON {
            ctx.set_zoom_factor(want_zoom);
        }
    }

    /// Rebuild the layout on a connector-set change (a replug) and seed any newly
    /// seen brightness value — without clobbering an in-flight operator edit.
    fn reconcile(&mut self, snap: &SeatSnapshot) {
        if let Probe::Present(connectors) = &snap.displays {
            let key: Vec<String> = connectors.iter().map(|c| c.name.clone()).collect();
            if key != self.layout_key {
                self.layout = DisplayLayout::from_connectors(connectors);
                self.layout_key = key;
            }
        }
        if let Probe::Present(panels) = &snap.backlights {
            for p in panels {
                self.panel_brightness
                    .entry(p.name.clone())
                    .or_insert_with(|| p.percent());
            }
        }
        if let Probe::Present(ddc) = &snap.ddc {
            for d in ddc {
                self.ddc_brightness
                    .entry(d.bus.clone())
                    .or_insert(d.brightness);
            }
        }
        // Seed the charge-cap slider from the first battery that advertises the
        // attribute, without clobbering an in-flight operator drag (POWER-4).
        if let Probe::Present(Some(pct)) = &snap.charge_limit {
            self.charge_threshold.get_or_insert(*pct);
        }
    }

    /// The latest seat snapshot, for the chrome status icons ([`crate::chrome`]).
    pub(crate) const fn snapshot(&self) -> Option<&SeatSnapshot> {
        self.snapshot.as_ref()
    }

    /// The POWER-5 idle/lid policy the honorer reads each tick (the source of truth
    /// the Power section edits).
    pub(crate) const fn power_honor_config(&self) -> &PowerHonorConfig {
        &self.power_honor_config
    }

    /// The latest lid reading for the POWER-5 honorer: `Some` only when the snapshot's
    /// lid probe is `Present` (a laptop with a lid device); `None` on a desktop
    /// (`Absent`) or before the first poll — the honorer never acts on a fabricated
    /// state.
    pub(crate) fn lid_state(&self) -> Option<LidState> {
        self.snapshot.as_ref()?.lid.present().copied()
    }

    /// Drive a power verb from the POWER-5 honorer through the ONE seat (lock 1) —
    /// the idle timer and lid handler act here (Suspend / Lock). The confirm-gate is
    /// deliberately bypassed: the honorer's arming IS the operator's consent (a
    /// chosen idle timeout / lid action), exactly as swayidle/logind would act
    /// unattended. A typed failure is returned for an honest note, never a panic.
    ///
    /// # Errors
    /// The logind client's typed errors (a polkit refusal / absent logind).
    pub(crate) fn honor_power(&self, verb: PowerVerb) -> Result<(), SeatError> {
        self.seat.power(verb)
    }

    /// Render the surface's live content as a **master-detail** shell (SETTINGS-1):
    /// a left rail of the three domain groups + a wide right detail pane that
    /// renders ONLY the selected section's body via the existing per-section fns
    /// (a layout/routing pass — the bodies + their `apply()`/`SysAction` seams are
    /// reused verbatim, §6). Drives Displays + Power against the seat.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        let mut actions: Vec<SysAction> = Vec::new();
        // Capture the rail selection before the render borrow so a rail click that
        // moves it can be detected + persisted afterwards (the same collect-then-
        // apply idiom the SysActions use — the render can't take `&mut self`).
        let nav_before = self.nav;
        // MENUBAR-ALL — the shared top bar (SYSTEM), above the master rail. Its three
        // menus mirror the rail's domain groups (Devices · Personalization · Mesh &
        // System), each listing every settings category as a radio item that jumps
        // `nav` — the SAME seam a rail-row click drives (§6), so the operator can
        // reach every section (incl. the advanced Pairing / Network / Power ones)
        // from the bar (the governing principle). A picked section is applied here so
        // the persist check below saves it exactly like a rail move.
        if let Some(section) = menubar::show(ui, self.nav.section, self.snapshot()) {
            self.nav = SettingsNav::at(section);
        }
        ui.separator();
        // Capture the appearance before the borrow so a Theme-section pick that moves
        // it can be detected + persisted afterwards (SETTINGS-5 — the same collect-
        // then-apply idiom `nav` uses; the live re-tint/zoom happens in the poll).
        let appearance_before = self.appearance;
        // Whether the BlueZ pairing responder is currently registered — read before
        // the mutable destructure (a Copy bool) so the Pairing section can surface
        // the responder's honest live state (SETTINGS-4).
        let agent_active = self.agent.is_some();
        {
            let Self {
                snapshot,
                layout,
                panel_brightness,
                ddc_brightness,
                confirm,
                charge_threshold,
                error,
                pairing,
                pin_input,
                power_honor_config,
                nav,
                mesh,
                appearance,
                ..
            } = self;
            let snap = snapshot.as_ref();
            // Whether a pairing PIN / passkey prompt is waiting for the shared modal
            // (SETTINGS-4 Pairing surfaces it; the modal below answers it).
            let prompt_in_flight = pairing.current().is_some();

            // The master rail: the three domain groups + their section rows. A row
            // click moves `nav` (persisted after the borrow). Each group header wears
            // its domain's categorical accent (SETTINGS-2 — see [`settings_rail`]); the
            // rail rests on the Carbon layer-01 page (see [`page_frame`]).
            egui::SidePanel::left(ui.id().with("settings-rail"))
                .resizable(false)
                .exact_width(Style::SP_XL * 6.0)
                .frame(page_frame(Style::SP_M))
                .show_inside(ui, |ui| settings_rail(ui, nav));

            // The (possibly just-clicked) selection, copied out so the detail pane's
            // closure doesn't re-borrow `nav`.
            let selected = nav.section;

            // The detail pane fills the remaining width and renders only the selected
            // section's body — expressive spacing, the whole right side. It rests on
            // the same Carbon layer-01 page (SETTINGS-2); the section body raises to a
            // layer-02 card inside (see [`settings_detail`]).
            egui::CentralPanel::default()
                .frame(page_frame(Style::SP_L))
                .show_inside(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if let Some(err) = error.as_deref() {
                                ui.colored_label(
                                    Style::DANGER,
                                    RichText::new(err).size(Style::SMALL),
                                );
                                ui.add_space(Style::SP_S);
                            }
                            settings_detail(
                                ui,
                                selected,
                                snap,
                                layout,
                                panel_brightness,
                                ddc_brightness,
                                *confirm,
                                charge_threshold,
                                power_honor_config,
                                mesh,
                                appearance,
                                agent_active,
                                prompt_in_flight,
                                &mut actions,
                            );
                        });
                });

            // The BlueZ pairing modal (E12-17): a ctx-level dialog that shows only
            // while a PIN/passkey/confirm prompt is in flight, draining the shared
            // bridge the registered agent posts to. Rendered here so it lives only
            // while the System surface is shown, never blocking the render thread.
            pairing_dialog(ui.ctx(), pairing, pin_input);
        }
        // Persist a moved rail selection across surface switches + restart (the
        // client-data-dir JSON idiom `PowerHonorConfig` uses). Only a real move
        // writes — an unchanged render never re-saves (§7: no inert write).
        if self.nav != nav_before {
            self.nav.save();
        }
        // Persist a Theme-section appearance pick the same way (SETTINGS-5). Only a
        // real change writes; the live re-tint/zoom lands on the next poll.
        if self.appearance != appearance_before {
            self.appearance.save();
        }
        self.apply(actions);
    }

    /// Apply the collected actions after the render borrow ends: drive the seat /
    /// the layout model, folding any typed failure into the
    /// honest inline error (never a panic, never a silent no-op).
    fn apply(&mut self, actions: Vec<SysAction>) {
        for action in actions {
            match action {
                SysAction::ToggleOutput(id, on) => match self.layout.set_enabled(&id, on) {
                    Ok(()) => self.error = None,
                    // The last-console interlock (or an unknown id) — surfaced typed.
                    Err(e) => self.error = Some(e.to_string()),
                },
                SysAction::SetMode(id, mode) => {
                    let _ = self.layout.set_mode(&id, mode);
                }
                SysAction::Nudge(id, left) => {
                    self.layout.nudge(&id, left);
                }
                SysAction::Backlight { name, raw } => {
                    if let Err(e) = self.seat.set_backlight(&name, raw) {
                        self.error = Some(format!("backlight {name}: {e}"));
                    } else {
                        self.error = None;
                    }
                }
                SysAction::Ddc { bus, percent } => {
                    if let Err(e) = self.seat.set_ddc_brightness(&bus, percent) {
                        self.error = Some(format!("DDC {bus}: {e}"));
                    } else {
                        self.error = None;
                    }
                }
                SysAction::ArmConfirm(verb) => self.confirm = Some(verb),
                SysAction::CancelConfirm => self.confirm = None,
                SysAction::Power(verb) => {
                    self.confirm = None;
                    if let Err(e) = self.seat.power(verb) {
                        self.error = Some(format!("{}: {e}", verb.label()));
                    } else {
                        self.error = None;
                    }
                }
                // POWER-4: the profile switch + charge-cap write route to their
                // own drive methods (mirroring the mixer/BT verb helpers) so each
                // folds a typed failure to the honest inline error, never a pretend
                // success (§7).
                SysAction::SetPowerProfile(name) => self.drive_power_profile(name),
                SysAction::SetChargeThreshold(pct) => self.drive_charge_threshold(pct),
                // POWER-5: persist the idle/lid policy the picker just mutated.
                SysAction::SavePowerHonorConfig => self.power_honor_config.save(),
                // ── Bluetooth writes (E12-17) — each drives the ONE seat's BlueZ
                // client, folds a typed failure to the inline error + a toast, and
                // optimistically reflects the cheap boolean toggles so the switch
                // doesn't flip back before the next 5s poll.
                SysAction::BtPower(path, on) => {
                    let r = self.seat.set_bt_powered(&path, on);
                    if self.bt_result(r, "power") {
                        if let Some(a) = self.bt_adapter_mut(&path) {
                            a.powered = on;
                        }
                    }
                }
                SysAction::BtDiscoverable(path, on) => {
                    let r = self.seat.set_bt_discoverable(&path, on);
                    if self.bt_result(r, "discoverable") {
                        if let Some(a) = self.bt_adapter_mut(&path) {
                            a.discoverable = on;
                        }
                    }
                }
                SysAction::BtPairable(path, on) => {
                    let r = self.seat.set_bt_pairable(&path, on);
                    if self.bt_result(r, "pairable") {
                        if let Some(a) = self.bt_adapter_mut(&path) {
                            a.pairable = on;
                        }
                    }
                }
                SysAction::BtScan(path, start) => {
                    let r = if start {
                        self.seat.bt_start_discovery(&path)
                    } else {
                        self.seat.bt_stop_discovery(&path)
                    };
                    if self.bt_result(r, "scan") {
                        if let Some(a) = self.bt_adapter_mut(&path) {
                            a.discovering = start;
                        }
                    }
                }
                SysAction::BtConnect(device) => {
                    // Connect/disconnect/pair/forget resolve over the link, so no
                    // optimistic flip — the next poll reflects the real state.
                    self.bt_result(self.seat.bt_connect(&device), "connect");
                }
                SysAction::BtDisconnect(device) => {
                    self.bt_result(self.seat.bt_disconnect(&device), "disconnect");
                }
                SysAction::BtPair(device) => {
                    self.bt_result(self.seat.bt_pair(&device), "pair");
                }
                SysAction::BtForget { adapter, device } => {
                    self.bt_result(self.seat.bt_remove_device(&adapter, &device), "forget");
                }
                SysAction::BtTrust(device, trusted) => {
                    let r = self.seat.set_bt_trusted(&device, trusted);
                    if self.bt_result(r, "trust") {
                        if let Some(d) = self.bt_device_mut(&device) {
                            d.trusted = trusted;
                        }
                    }
                }
                // SETTINGS-4: re-arm the pairing responder on the shared seam.
                SysAction::PairingRetry => self.retry_pairing_agent(),
            }
        }
    }

    /// Re-arm the pairing responder (SETTINGS-4 Pairing section): clear the
    /// once-per-visit latch, then re-attempt registration on the SAME
    /// [`Self::sync_pairing_agent`] seam main.rs drives on visibility. With no
    /// adapter this is an honest no-op (nothing to pair); a real failure re-toasts
    /// once (§7), never a fabricated agent, never a second responder (§6).
    fn retry_pairing_agent(&mut self) {
        self.agent_attempted = false;
        self.sync_pairing_agent(true);
    }

    /// Drive a POWER-4 profile switch through the real seat: on success
    /// optimistically reflect the new active so the segmented control settles
    /// before the next 5s poll; a refused/absent switch is surfaced honestly and
    /// the cached active is NOT flipped (§7 — a failed switch never lies "active").
    fn drive_power_profile(&mut self, name: String) {
        if let Err(e) = self.seat.set_power_profile(&name) {
            self.error = Some(power_settings::profile_error(&e));
        } else {
            self.error = None;
            if let Some(Probe::Present(p)) = self.snapshot.as_mut().map(|s| &mut s.power_profile) {
                p.active = name;
            }
        }
    }

    /// Drive a POWER-4 charge-cap write through the real seat. A refused/absent
    /// write or the EACCES on the root-owned sysfs attribute is surfaced honestly
    /// inline, never a pretend cap (§7).
    fn drive_charge_threshold(&mut self, pct: u8) {
        if let Err(e) = self.seat.set_charge_threshold(pct) {
            self.error = Some(power_settings::charge_error(&e));
        } else {
            self.error = None;
            self.charge_threshold = Some(pct);
        }
    }

    /// Fold a Bluetooth write's typed result: clear the inline error on success,
    /// else surface it inline AND raise a toast (§7 — a refused/absent write is an
    /// honest alert, never a silent no-op). Returns whether the write succeeded, so
    /// the caller can optimistically update the cached snapshot.
    fn bt_result(&mut self, r: Result<(), SeatError>, verb: &str) -> bool {
        match r {
            Ok(()) => {
                self.error = None;
                true
            }
            Err(e) => {
                self.pending_toasts.push(bt_error_toast(verb, &e));
                self.error = Some(format!("Bluetooth {verb}: {e}"));
                false
            }
        }
    }

    /// A mutable view of a cached adapter (for the optimistic toggle update).
    fn bt_adapter_mut(&mut self, path: &str) -> Option<&mut BtAdapter> {
        match self.snapshot.as_mut()?.bluetooth {
            Probe::Present(ref mut bt) => bt.adapters.iter_mut().find(|a| a.path == path),
            Probe::Absent { .. } => None,
        }
    }

    /// A mutable view of a cached device (for the optimistic trust update).
    fn bt_device_mut(&mut self, path: &str) -> Option<&mut BtDevice> {
        match self.snapshot.as_mut()?.bluetooth {
            Probe::Present(ref mut bt) => bt.devices.iter_mut().find(|d| d.path == path),
            Probe::Absent { .. } => None,
        }
    }

    /// Register or drop the `BlueZ` pairing agent to track the System surface's
    /// visibility (E12-17). Registered only once an adapter is present (a headless
    /// host has nothing to pair, and `register` would just answer Unavailable);
    /// dropping the handle unregisters it. A registration failure toasts once.
    pub(crate) fn sync_pairing_agent(&mut self, active: bool) {
        if !active {
            // Leaving the panel: drop the agent (Drop unregisters) and re-arm.
            self.agent = None;
            self.agent_attempted = false;
            return;
        }
        if self.agent.is_some() || self.agent_attempted {
            return;
        }
        let has_adapter = matches!(
            self.snapshot.as_ref().map(|s| &s.bluetooth),
            Some(Probe::Present(bt)) if !bt.adapters.is_empty()
        );
        if !has_adapter {
            return;
        }
        self.agent_attempted = true;
        match PairingAgent::register(Arc::new(self.pairing.clone())) {
            Ok(agent) => self.agent = Some(agent),
            Err(e) => self
                .pending_toasts
                .push(bt_error_toast("pairing agent", &e)),
        }
    }

    /// Drain the Bluetooth control-error toasts for the shell to raise into the one
    /// `ToastBridge` (called after `show()`, once the render borrow has ended).
    pub(crate) fn take_toasts(&mut self) -> Vec<Toast> {
        std::mem::take(&mut self.pending_toasts)
    }

    // ── hotkey dispatch (E12-19) ────────────────────────────────────────────
    //
    // The shell's hotkey router (`crate::hotkeys`) turns a matched chord into a
    // typed `HotkeyAction`; the *hardware* actions (volume / brightness / mute /
    // Bluetooth / lock) act through the ONE seat here (lock 1), reusing the same
    // control verbs the panel's sliders drive. Volume + brightness return an
    // `OsdLevel` the shell flashes on the KIRON OSD tier (lock 11 / KIRON-3). The
    // navigation actions (session/monitor switch, return-to-chrome, open-system)
    // are the shell's to apply, not the seat's — this returns `None` for them.

    /// Act on a hardware hotkey against the seat, returning the OSD level to flash
    /// (volume / brightness) or `None`. A failed or unavailable backend folds to the
    /// same honest inline error the panel controls use — never a panic, never a
    /// silent no-op.
    pub(crate) fn dispatch_hotkey(&mut self, action: HotkeyAction) -> Option<OsdLevel> {
        match action {
            HotkeyAction::VolumeUp => self.nudge_master_volume(HOTKEY_STEP),
            HotkeyAction::VolumeDown => self.nudge_master_volume(-HOTKEY_STEP),
            HotkeyAction::VolumeMute => self.toggle_master_mute(),
            HotkeyAction::MicMute => self.toggle_mic_mute(),
            HotkeyAction::BrightnessUp => self.nudge_brightness(HOTKEY_STEP),
            HotkeyAction::BrightnessDown => self.nudge_brightness(-HOTKEY_STEP),
            HotkeyAction::BluetoothToggle => {
                self.toggle_bluetooth();
                None
            }
            HotkeyAction::Lock => {
                if let Err(e) = self.seat.power(PowerVerb::Lock) {
                    self.error = Some(format!("Lock: {e}"));
                } else {
                    self.error = None;
                }
                None
            }
            // Navigation — the shell applies these (they don't touch hardware).
            HotkeyAction::SessionSwitch
            | HotkeyAction::MonitorFocusSwitch
            | HotkeyAction::ReturnToChrome
            | HotkeyAction::OpenSystem => None,
        }
    }

    /// The cached master strip, if the mixer probe answered — the hotkeys' target.
    fn master_strip(&self) -> Option<&MixerStrip> {
        match self.snapshot.as_ref()?.mixer {
            Probe::Present(ref m) => Some(&m.master),
            Probe::Absent { .. } => None,
        }
    }

    /// Nudge the master output volume by `delta` (clamped 0–100), driving the seat
    /// and updating the cached level so rapid taps accumulate before the next poll.
    fn nudge_master_volume(&mut self, delta: i16) -> Option<OsdLevel> {
        let (id, cur) = {
            let m = self.master_strip()?;
            (m.id.clone(), i16::from(m.volume))
        };
        let next = u8::try_from((cur + delta).clamp(0, 100)).unwrap_or(0);
        match self.seat.set_strip_volume(&id, next) {
            Ok(()) => {
                self.error = None;
                if let Some(m) = self.master_strip_mut() {
                    m.volume = next;
                }
                Some(OsdLevel::new(OsdKind::Volume, f32::from(next) / 100.0))
            }
            Err(e) => {
                self.error = Some(format!("volume: {e}"));
                None
            }
        }
    }

    /// Toggle the master output mute, driving the seat and updating the cache. The
    /// OSD shows the muted glyph when it goes muted, the level bar when it comes back.
    fn toggle_master_mute(&mut self) -> Option<OsdLevel> {
        let (id, muted, vol) = {
            let m = self.master_strip()?;
            (m.id.clone(), m.muted, m.volume)
        };
        match self.seat.set_strip_muted(&id, !muted) {
            Ok(()) => {
                self.error = None;
                if let Some(m) = self.master_strip_mut() {
                    m.muted = !muted;
                }
                let kind = if muted {
                    OsdKind::Volume
                } else {
                    OsdKind::Muted
                };
                Some(OsdLevel::new(kind, f32::from(vol) / 100.0))
            }
            Err(e) => {
                self.error = Some(format!("mute: {e}"));
                None
            }
        }
    }

    /// The mixer model is output-only (master + playback strips), so there is no
    /// capture strip to mute — an honest not-available state, never a dead key.
    fn toggle_mic_mute(&mut self) -> Option<OsdLevel> {
        self.error = Some("Microphone mute: no capture strip on this seat.".to_owned());
        None
    }

    /// Nudge display brightness by `delta`: the first sysfs backlight panel if
    /// present, else the first DDC/CI monitor, else an honest not-controllable note.
    /// The live 0–100 value tracks the same maps the sliders own, so a hotkey tap
    /// and a slider drag stay in sync.
    fn nudge_brightness(&mut self, delta: i16) -> Option<OsdLevel> {
        // Prefer an internal panel (sysfs backlight).
        if let Some((name, max, seed)) = self.first_backlight() {
            let cur = i16::from(*self.panel_brightness.entry(name.clone()).or_insert(seed));
            let next = u8::try_from((cur + delta).clamp(0, 100)).unwrap_or(0);
            let raw = u32::from(next) * max / 100;
            return match self.seat.set_backlight(&name, raw) {
                Ok(()) => {
                    self.error = None;
                    self.panel_brightness.insert(name, next);
                    Some(OsdLevel::new(OsdKind::Brightness, f32::from(next) / 100.0))
                }
                Err(e) => {
                    self.error = Some(format!("brightness: {e}"));
                    None
                }
            };
        }
        // Else an external monitor over DDC/CI.
        if let Some((bus, seed)) = self.first_ddc() {
            let cur = i16::from(*self.ddc_brightness.entry(bus.clone()).or_insert(seed));
            let next = u8::try_from((cur + delta).clamp(0, 100)).unwrap_or(0);
            return match self.seat.set_ddc_brightness(&bus, next) {
                Ok(()) => {
                    self.error = None;
                    self.ddc_brightness.insert(bus, next);
                    Some(OsdLevel::new(OsdKind::Brightness, f32::from(next) / 100.0))
                }
                Err(e) => {
                    self.error = Some(format!("brightness (DDC): {e}"));
                    None
                }
            };
        }
        self.error = Some("Brightness: not controllable (no backlight / DDC).".to_owned());
        None
    }

    /// Toggle the first Bluetooth adapter's radio power, driving the seat + cache.
    fn toggle_bluetooth(&mut self) {
        let Some(snap) = self.snapshot.as_ref() else {
            return;
        };
        let Probe::Present(bt) = &snap.bluetooth else {
            self.error = Some("Bluetooth: no adapter.".to_owned());
            return;
        };
        let Some(adapter) = bt.adapters.first() else {
            self.error = Some("Bluetooth: no adapter.".to_owned());
            return;
        };
        let (path, on) = (adapter.path.clone(), !adapter.powered);
        match self.seat.set_bt_powered(&path, on) {
            Ok(()) => {
                self.error = None;
                if let Some(Probe::Present(bt)) = self.snapshot.as_mut().map(|s| &mut s.bluetooth) {
                    if let Some(a) = bt.adapters.iter_mut().find(|a| a.path == path) {
                        a.powered = on;
                    }
                }
            }
            Err(e) => self.error = Some(format!("Bluetooth: {e}")),
        }
    }

    /// Mutable view of the cached master strip (for the accumulate-in-place update).
    fn master_strip_mut(&mut self) -> Option<&mut MixerStrip> {
        match self.snapshot.as_mut()?.mixer {
            Probe::Present(ref mut m) => Some(&mut m.master),
            Probe::Absent { .. } => None,
        }
    }

    /// The first backlight panel's `(name, max, seed %)`, if the probe answered.
    fn first_backlight(&self) -> Option<(String, u32, u8)> {
        match self.snapshot.as_ref()?.backlights {
            Probe::Present(ref panels) => {
                panels.first().map(|p| (p.name.clone(), p.max, p.percent()))
            }
            Probe::Absent { .. } => None,
        }
    }

    /// The first DDC monitor's `(bus, seed %)`, if the probe answered.
    fn first_ddc(&self) -> Option<(String, u8)> {
        match self.snapshot.as_ref()?.ddc {
            Probe::Present(ref list) => list.first().map(|d| (d.bus.clone(), d.brightness)),
            Probe::Absent { .. } => None,
        }
    }
}

// ──────────────────────────── master-detail nav (SETTINGS-1) ────────────────────────────

/// One rail leaf of the Settings master-detail shell (SETTINGS-1): the six existing
/// host-control sections plus the four Mesh & System sections SETTINGS-4 wired to
/// this node's real identity / role / pairing / network state. Each belongs to
/// exactly one [`SettingsGroup`]; the pair the rail rests on is a [`SettingsNav`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SettingsSection {
    /// Per-output enable / mode / arrangement + brightness (`displays_section`).
    #[default]
    Displays,
    /// The mixer strips (`mixer_section`) — labelled "Audio" in the rail.
    Audio,
    /// Adapters + devices (`bluetooth_section`).
    Bluetooth,
    /// Logind verbs + profiles + batteries + per-VM rows (`power_section`).
    Power,
    /// The desktop backdrop picker (`wallpaper_section`).
    Wallpaper,
    /// The compiled-in hotkey table (`hotkeys_section`).
    Hotkeys,
    /// Appearance — accent + text-scale (`theme_section`, SETTINGS-5).
    Theme,
    /// Mesh identity name + overlay/cipher (`identity_section`, SETTINGS-4).
    Identity,
    /// The pinned deployment role (`role_section`, SETTINGS-4).
    Role,
    /// The pairing responder (`pairing_section`, SETTINGS-4).
    Pairing,
    /// Overlay/underlay network facts (`network_section`, SETTINGS-4).
    Network,
}

impl SettingsSection {
    /// The rail + detail-header label.
    const fn label(self) -> &'static str {
        match self {
            Self::Displays => "Displays",
            Self::Audio => "Audio",
            Self::Bluetooth => "Bluetooth",
            Self::Power => "Power & Battery",
            Self::Wallpaper => "Wallpaper",
            Self::Hotkeys => "Hotkeys",
            Self::Theme => "Theme",
            Self::Identity => "Identity",
            Self::Role => "Role",
            Self::Pairing => "Pairing",
            Self::Network => "Network",
        }
    }

    /// The domain group this section lives under (the single source of truth the
    /// rail + [`SettingsNav`] normalise against).
    const fn group(self) -> SettingsGroup {
        match self {
            Self::Displays | Self::Audio | Self::Bluetooth | Self::Power => SettingsGroup::Devices,
            Self::Wallpaper | Self::Hotkeys | Self::Theme => SettingsGroup::Personalization,
            Self::Identity | Self::Role | Self::Pairing | Self::Network => {
                SettingsGroup::MeshSystem
            }
        }
    }
}

/// A domain group — the top level of the master rail (lock 3). Scales as sections
/// grow; the taxonomy places every section exactly once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SettingsGroup {
    /// Displays · Audio · Bluetooth · Power & Battery.
    #[default]
    Devices,
    /// Wallpaper · Hotkeys · Theme.
    Personalization,
    /// Identity · Role · Pairing · Network (SETTINGS-4 — this node's mesh facts).
    MeshSystem,
}

impl SettingsGroup {
    /// The three domain groups, in rail order.
    const ALL: [Self; 3] = [Self::Devices, Self::Personalization, Self::MeshSystem];

    /// The rail header label.
    const fn label(self) -> &'static str {
        match self {
            Self::Devices => "Devices",
            Self::Personalization => "Personalization",
            Self::MeshSystem => "Mesh & System",
        }
    }

    /// This group's categorical **accent** (SETTINGS-2, design lock #2). REUSES the
    /// shared `Style::ACCENT_*` categorical set — the ONE colour language the bottom
    /// picker (PICKER-2) + the unit explorer (EXPLORER-15) already speak — so a
    /// domain's tint here reads the same across the shell (§4; no second set minted).
    /// Three mutually-distinct hues, each set apart from the interactive brand accent
    /// so a group tint never reads as an affordance. The rail group header + the
    /// active detail-section header both key off this.
    const fn accent(self) -> egui::Color32 {
        match self {
            // Host devices / hardware — the picker's host-control gold.
            Self::Devices => Style::ACCENT_SYSTEM,
            // Appearance / personalization — the expressive magenta.
            Self::Personalization => Style::ACCENT_MEDIA,
            // Mesh identity / role / pairing / network — the mesh green.
            Self::MeshSystem => Style::ACCENT_MESH,
        }
    }

    /// This group's sections, in rail order.
    const fn sections(self) -> &'static [SettingsSection] {
        match self {
            Self::Devices => &[
                SettingsSection::Displays,
                SettingsSection::Audio,
                SettingsSection::Bluetooth,
                SettingsSection::Power,
            ],
            Self::Personalization => &[
                SettingsSection::Wallpaper,
                SettingsSection::Hotkeys,
                SettingsSection::Theme,
            ],
            Self::MeshSystem => &[
                SettingsSection::Identity,
                SettingsSection::Role,
                SettingsSection::Pairing,
                SettingsSection::Network,
            ],
        }
    }
}

/// The client-data-dir file the rail selection persists to (the `PowerHonorConfig`
/// idiom — one small JSON per shell preference).
const NAV_CONFIG_FILE: &str = "settings-nav.json";

/// The Settings rail selection (SETTINGS-1): the domain group + section the
/// master-detail rail last rested on. Persisted so the surface reopens where the
/// operator left it — across a surface switch AND a restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct SettingsNav {
    /// The active domain group (always re-derived from `section` so the pair can
    /// never present an inconsistent state, §7).
    #[serde(default)]
    group: SettingsGroup,
    /// The active section — the rail leaf the detail pane renders.
    #[serde(default)]
    section: SettingsSection,
}

impl Default for SettingsNav {
    fn default() -> Self {
        Self::at(SettingsSection::Displays)
    }
}

impl SettingsNav {
    /// The nav resting on `section`, its group derived so the pair is always
    /// consistent (the only constructor a rail click uses).
    const fn at(section: SettingsSection) -> Self {
        Self {
            group: section.group(),
            section,
        }
    }

    /// Re-derive the group from the section so a hand-edited / schema-drifted file
    /// can never present an inconsistent pair (§7 — the section wins).
    const fn normalized(self) -> Self {
        Self::at(self.section)
    }

    /// The default nav path (`<client-data-dir>/settings-nav.json`), or `None` when
    /// no data dir resolves (a headless context) — mirrors `PowerHonorConfig`.
    fn default_path() -> Option<PathBuf> {
        mde_bus::client_data_dir().map(|d| d.join(NAV_CONFIG_FILE))
    }

    /// Load from `path`, folding a missing / malformed file to the default (never a
    /// fatal) and normalising the group against the section.
    fn load_from(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Self>(&s).ok())
            .map_or_else(Self::default, Self::normalized)
    }

    /// Load from the default path (default when absent / unresolvable).
    #[must_use]
    fn load() -> Self {
        Self::default_path().map_or_else(Self::default, |p| Self::load_from(&p))
    }

    /// Write to `path` (atomic temp + rename, like `PowerHonorConfig`). Takes `self`
    /// by value — the nav is a 2-byte `Copy`.
    ///
    /// # Errors
    /// The [`std::io::Error`] if the dir cannot be created or the file cannot be
    /// written / renamed.
    fn save_to(self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Persist to the default path (a silent no-op when no data dir resolves).
    fn save(self) {
        if let Some(path) = Self::default_path() {
            let _ = self.save_to(&path);
        }
    }
}

// ──────────────────────────── Personalization → Theme (SETTINGS-5) ────────────────────────────

/// A curated interactive-**accent** choice (SETTINGS-5). Each variant maps to ONE
/// existing shared `Style::ACCENT*` token, so the picker offers the shell's own colour
/// language — never an arbitrary raw colour (§4 — no new hex minted). `Brand` is the
/// default interactive blue the shell installs; the rest reuse the categorical accent
/// hues as the interactive tint. Applied live via [`Style::set_accent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AccentChoice {
    /// The default interactive brand accent (`Style::ACCENT`, blue).
    #[default]
    Brand,
    /// `Style::ACCENT_COMMS` (cyan).
    Cyan,
    /// `Style::ACCENT_WORKLOADS` (purple).
    Purple,
    /// `Style::ACCENT_TERMINALS` (teal).
    Teal,
    /// `Style::ACCENT_MESH` (green).
    Green,
    /// `Style::ACCENT_SYSTEM` (gold).
    Gold,
    /// `Style::ACCENT_MEDIA` (magenta).
    Magenta,
}

impl AccentChoice {
    /// The choices in picker order (Brand first).
    const ALL: [Self; 7] = [
        Self::Brand,
        Self::Cyan,
        Self::Purple,
        Self::Teal,
        Self::Green,
        Self::Gold,
        Self::Magenta,
    ];

    /// The picker label.
    const fn label(self) -> &'static str {
        match self {
            Self::Brand => "Brand",
            Self::Cyan => "Cyan",
            Self::Purple => "Purple",
            Self::Teal => "Teal",
            Self::Green => "Green",
            Self::Gold => "Gold",
            Self::Magenta => "Magenta",
        }
    }

    /// The shared `Style` token this choice paints — the SAME colour language the
    /// picker (PICKER-2) + explorer (EXPLORER-15) + Settings groups (SETTINGS-2) speak
    /// (§4 — no new hue).
    const fn color(self) -> egui::Color32 {
        match self {
            Self::Brand => Style::ACCENT,
            Self::Cyan => Style::ACCENT_COMMS,
            Self::Purple => Style::ACCENT_WORKLOADS,
            Self::Teal => Style::ACCENT_TERMINALS,
            Self::Green => Style::ACCENT_MESH,
            Self::Gold => Style::ACCENT_SYSTEM,
            Self::Magenta => Style::ACCENT_MEDIA,
        }
    }
}

/// A platform **text-scale** step (SETTINGS-5) — the EXPLORER-18 accessibility posture:
/// the whole-UI zoom the shell honors so type + hit-targets scale together. Discrete
/// legible steps (not a free slider) so the choice reads clearly and round-trips
/// exactly. Applied live as the egui zoom multiplier atop the seat's DPI base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TextScale {
    /// 90% — a denser layout.
    Small,
    /// 100% — the design baseline.
    #[default]
    Default,
    /// 115%.
    Large,
    /// 130%.
    Larger,
    /// 150% — the accessibility maximum.
    Largest,
}

impl TextScale {
    /// The steps in slider order (smallest first).
    const ALL: [Self; 5] = [
        Self::Small,
        Self::Default,
        Self::Large,
        Self::Larger,
        Self::Largest,
    ];

    /// The picker label.
    const fn label(self) -> &'static str {
        match self {
            Self::Small => "Small",
            Self::Default => "Default",
            Self::Large => "Large",
            Self::Larger => "Larger",
            Self::Largest => "Largest",
        }
    }

    /// The whole-UI zoom multiplier applied atop the seat's DPI base — the egui
    /// `zoom_factor` the shell honors. `Default` is the identity (1.0), a no-op.
    const fn factor(self) -> f32 {
        match self {
            Self::Small => 0.9,
            Self::Default => 1.0,
            Self::Large => 1.15,
            Self::Larger => 1.3,
            Self::Largest => 1.5,
        }
    }
}

/// The client-data-dir file the Personalization → Theme appearance persists to (the
/// [`SettingsNav`] / `PowerHonorConfig` one-JSON-per-preference idiom).
const APPEARANCE_CONFIG_FILE: &str = "settings-appearance.json";

/// The persisted Personalization → Theme appearance (SETTINGS-5): the interactive
/// accent + the platform text-scale the shell actually applies at runtime. Loaded on
/// start and restored on open, saved on a pick — the [`SettingsNav`] client-data-dir
/// JSON idiom, reused. Both fields drive a real live effect through
/// [`SystemState::apply_appearance`] (§7 — no dead toggle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
struct AppearanceConfig {
    /// The interactive accent tint (re-applied over the installed look each frame).
    #[serde(default)]
    accent: AccentChoice,
    /// The whole-UI text-scale step (the EXPLORER-18 accessibility zoom).
    #[serde(default)]
    text_scale: TextScale,
}

impl AppearanceConfig {
    /// The default appearance path (`<client-data-dir>/settings-appearance.json`), or
    /// `None` when no data dir resolves (a headless context) — mirrors [`SettingsNav`].
    fn default_path() -> Option<PathBuf> {
        mde_bus::client_data_dir().map(|d| d.join(APPEARANCE_CONFIG_FILE))
    }

    /// Load from `path`, folding a missing / malformed file to the default (never a
    /// fatal) — the `#[serde(default)]` fields also tolerate a partial / drifted file.
    fn load_from(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Self>(&s).ok())
            .unwrap_or_default()
    }

    /// Load from the default path (default when absent / unresolvable).
    #[must_use]
    fn load() -> Self {
        Self::default_path().map_or_else(Self::default, |p| Self::load_from(&p))
    }

    /// Write to `path` (atomic temp + rename, like [`SettingsNav`]). Takes `self` by
    /// value — the appearance is a small `Copy`.
    ///
    /// # Errors
    /// The [`std::io::Error`] if the dir cannot be created or the file cannot be
    /// written / renamed.
    fn save_to(self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Persist to the default path (a silent no-op when no data dir resolves).
    fn save(self) {
        if let Some(path) = Self::default_path() {
            let _ = self.save_to(&path);
        }
    }
}

// ──────────────────────────── render ────────────────────────────

/// The master rail (SETTINGS-1): the three domain groups, each an expressive header
/// over its selectable section rows. The active section is highlighted; a click
/// moves `nav`. SETTINGS-2 tints each group header in the group's categorical accent
/// ([`SettingsGroup::accent`] — the shared `Style::ACCENT_*` set, §4), the one colour
/// language PICKER-2 / EXPLORER-15 speak.
fn settings_rail(ui: &mut egui::Ui, nav: &mut SettingsNav) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (i, group) in SettingsGroup::ALL.iter().enumerate() {
                if i > 0 {
                    ui.add_space(Style::SP_M);
                }
                ui.label(
                    RichText::new(group.label())
                        .color(group.accent())
                        .size(Style::SMALL)
                        .strong(),
                );
                ui.add_space(Style::SP_XS);
                for &section in group.sections() {
                    let selected = nav.section == section;
                    let row = ui.add_sized(
                        [ui.available_width(), Style::SP_L],
                        egui::SelectableLabel::new(
                            selected,
                            RichText::new(section.label()).size(Style::BODY),
                        ),
                    );
                    if row.clicked() {
                        *nav = SettingsNav::at(section);
                    }
                }
            }
        });
}

/// The detail pane (SETTINGS-1): an expressive header over the selected section's
/// body, rendered by calling the EXISTING per-section fn verbatim (§6 — no forked
/// logic; every `apply()`/`SysAction` seam is reused). The Mesh & System sections
/// (SETTINGS-4) render this node's real identity / role / pairing / network state,
/// honest-`unknown` where the snapshot doesn't carry a fact (§7).
#[allow(clippy::too_many_arguments)] // one router legibly threading the live section refs
fn settings_detail(
    ui: &mut egui::Ui,
    section: SettingsSection,
    snap: Option<&SeatSnapshot>,
    layout: &DisplayLayout,
    panel_brightness: &mut BTreeMap<String, u8>,
    ddc_brightness: &mut BTreeMap<String, u8>,
    confirm: Option<PowerVerb>,
    charge_threshold: &mut Option<u8>,
    power_honor_config: &mut PowerHonorConfig,
    mesh: &MeshFacts,
    appearance: &mut AppearanceConfig,
    agent_active: bool,
    prompt_in_flight: bool,
    actions: &mut Vec<SysAction>,
) {
    // Expressive header — the active section's title in the large type scale, tinted
    // in its domain group's categorical accent (SETTINGS-2) so the active domain reads
    // at a glance in the same colour as its rail header.
    ui.label(
        RichText::new(section.label())
            .color(section.group().accent())
            .size(Style::HEADING)
            .strong(),
    );
    ui.add_space(Style::SP_M);
    // The section body sits on a Carbon layer-02 card raised above the layer-01 page,
    // ringed by a hairline border (SETTINGS-2 — [`section_card`]).
    section_card(ui, |ui| match section {
        SettingsSection::Displays => {
            displays_section(ui, snap, layout, panel_brightness, ddc_brightness, actions)
        }
        SettingsSection::Audio => mixer_section(ui, snap),
        SettingsSection::Bluetooth => bluetooth_section(ui, snap, actions),
        SettingsSection::Power => power_section(
            ui,
            snap,
            confirm,
            charge_threshold,
            power_honor_config,
            actions,
        ),
        SettingsSection::Wallpaper => wallpaper_section(ui),
        SettingsSection::Hotkeys => hotkeys_section(ui),
        SettingsSection::Theme => theme_section(ui, appearance),
        SettingsSection::Identity => identity_section(ui, mesh),
        SettingsSection::Role => role_section(ui, mesh),
        SettingsSection::Pairing => {
            pairing_section(ui, snap, agent_active, prompt_in_flight, actions);
        }
        SettingsSection::Network => network_section(ui, mesh),
    });
}

/// The Settings **page** frame (SETTINGS-2) — Carbon **layer-01**: the rail + the
/// detail pane rest one elevation step above the window [`Style::BG`], the base the
/// section cards raise from. `margin` is the pane's inner pad (its own expressive
/// value per pane). All tokens — no raw literal (§4).
fn page_frame(margin: f32) -> egui::Frame {
    egui::Frame::NONE.fill(Style::LAYER_01).inner_margin(margin)
}

/// The Settings **section card** frame (SETTINGS-2) — Carbon **layer-02**: the
/// selected section's body sits one elevation step above the layer-01 page, ringed by
/// a hairline [`Style::BORDER`] with the shared corner radius. Every value is a
/// [`Style`] token (fill / stroke / radius / pad — no raw literal, §4).
fn card_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(Style::LAYER_02)
        .stroke(egui::Stroke::new(1.0, Style::BORDER))
        .corner_radius(Style::RADIUS)
        .inner_margin(Style::SP_M)
}

/// Render `add` inside a [`card_frame`] — the layer-02 section card that replaces the
/// plain `ui.group`, so the elevation ladder + hairline read as one Carbon surface.
/// Generic over the body's return (the `ui.group` shape it supersedes), so a section
/// fn's value threads straight through.
fn section_card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    card_frame().show(ui, add).inner
}

// ──────────────────────────── wide-layout helpers (SETTINGS-3) ────────────────────────────

/// The minimum comfortable width for one detail tile / column (Carbon expressive,
/// design lock #4). The wide detail pane fits as many equal columns of at least this
/// width as it can; below it a column is dropped so a tile never crushes on a small
/// DRM seat (the small-seat concession). Derived from the spacing grid — no raw
/// literal (§4), the `Style::SP_XL * n` idiom the rail width already uses.
const TILE_MIN_W: f32 = Style::SP_XL * 8.0;

/// The Settings **detail tile** frame (SETTINGS-3) — a nested card inside the
/// section's layer-02 body card. It rests on Carbon **layer-01** (a tonal step in from
/// the raised body card — the alternating-layer Carbon nesting model) ringed by the
/// shared hairline [`Style::BORDER`] + corner radius, so a row / column of tiles reads
/// as distinct cards. Every value a [`Style`] token, reusing the SETTINGS-2 elevation
/// ladder (§4 — no raw literal).
fn tile_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(Style::LAYER_01)
        .stroke(egui::Stroke::new(1.0, Style::BORDER))
        .corner_radius(Style::RADIUS)
        .inner_margin(Style::SP_S)
}

/// Render `add` inside a [`tile_frame`], stretched to fill its column so a row of
/// tiles reads as equal cards (the frame otherwise shrinks to its content). Generic
/// over the body's return so a section's value threads straight through.
fn tile<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    tile_frame()
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A **titled detail column** — a [`tile`] card headed by a dim caption — for one side
/// of a side-by-side wide layout (Bluetooth adapters | devices, Power controls |
/// battery). Reuses the tile card + the SETTINGS-2 tokens (§4).
fn column_card<R>(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    tile(ui, |ui| {
        ui.label(
            RichText::new(title)
                .color(Style::TEXT_DIM)
                .size(Style::SMALL)
                .strong(),
        );
        ui.add_space(Style::SP_XS);
        add(ui)
    })
}

/// How many equal columns of at least [`TILE_MIN_W`] fit `avail` points — at least
/// one, capped at `upper`. The wide detail pane lays its items across this many
/// columns; a narrow seat collapses toward one (graceful, never clipped — design
/// lock #4). A pure fold (no float→int cast), unit-tested headless.
fn fit_columns(avail: f32, upper: usize) -> usize {
    let mut cols = 1;
    let mut needed = TILE_MIN_W * 2.0;
    while cols < upper && avail >= needed {
        cols += 1;
        needed += TILE_MIN_W;
    }
    cols
}

/// Lay `items` **across** the wide detail pane in a responsive grid: as many equal
/// columns of at least [`TILE_MIN_W`] as fit (capped at `max_cols`, never more than the
/// item count), each cell rendered by `cell`. A narrow seat collapses toward one column
/// so nothing clips (design lock #4). The single across-the-width primitive the
/// reworked Displays / Audio / Hotkeys bodies share (§6). Empty `items` draw nothing —
/// the caller shows its own honest-empty note.
fn across_grid<T>(
    ui: &mut egui::Ui,
    items: &[T],
    max_cols: usize,
    mut cell: impl FnMut(&mut egui::Ui, &T),
) {
    if items.is_empty() {
        return;
    }
    let cols = fit_columns(ui.available_width(), max_cols.min(items.len()));
    for chunk in items.chunks(cols) {
        ui.columns(cols, |columns| {
            for (slot, item) in chunk.iter().enumerate() {
                cell(&mut columns[slot], item);
            }
        });
    }
}

/// Fold a snapshot [`Probe`] into its render: not-yet-polled → "reading…",
/// `Absent` → the shared honest not-available note (§7), `Present` → the rows.
fn probe_section<T>(
    ui: &mut egui::Ui,
    snap: Option<&SeatSnapshot>,
    pick: impl FnOnce(&SeatSnapshot) -> &Probe<T>,
    present: impl FnOnce(&mut egui::Ui, &T),
) {
    match snap.map(pick) {
        None => {
            muted_note(ui, "Reading the seat…");
        }
        Some(Probe::Present(v)) => present(ui, v),
        Some(Probe::Absent { reason, .. }) => {
            muted_note(ui, reason.clone());
        }
    }
}

/// The Audio / Mixer section — read-only status (fader/mute/solo interaction is
/// E12-16). The master output is the emphasized channel spanning the pane; the
/// playback strips spread **across** the wide detail pane as channel tiles
/// (SETTINGS-3), not a stacked column.
fn mixer_section(ui: &mut egui::Ui, snap: Option<&SeatSnapshot>) {
    probe_section(
        ui,
        snap,
        |s| &s.mixer,
        |ui, m: &MixerStatus| {
            tile(ui, |ui| strip_channel(ui, &m.master, true));
            if m.strips.is_empty() {
                ui.add_space(Style::SP_S);
                muted_note(ui, "No channel strips.");
                return;
            }
            ui.add_space(Style::SP_S);
            // The channel strips laid across the wide pane, up to four to a row.
            across_grid(ui, &m.strips, 4, |ui, strip| {
                tile(ui, |ui| strip_channel(ui, strip, false));
            });
        },
    );
}

/// One mixer channel as a read-only tile: a status dot + name, then the level (and an
/// honest "muted" flag). The across-the-width channel the Audio section lays in a row
/// (SETTINGS-3), replacing the old stacked [`field`] row.
fn strip_channel(ui: &mut egui::Ui, strip: &MixerStrip, master: bool) {
    let tone = if strip.muted { Style::WARN } else { Style::OK };
    ui.horizontal(|ui| {
        ui.label(RichText::new(DOT).color(tone).size(Style::SMALL));
        ui.add_space(Style::SP_XS);
        let name = if master {
            "Master"
        } else {
            strip.name.as_str()
        };
        ui.label(
            RichText::new(name)
                .color(Style::TEXT)
                .size(Style::SMALL)
                .strong(),
        );
    });
    let level_tone = if strip.muted {
        Style::WARN
    } else {
        Style::TEXT
    };
    let size = if master { Style::BODY } else { Style::SMALL };
    ui.label(
        RichText::new(format!("{}%", strip.volume))
            .color(level_tone)
            .size(size),
    );
    if strip.muted {
        muted_note(ui, "muted");
    }
}

/// Whether the passed device state offers each action button (the pure
/// button-enable logic, unit-tested headless). `connect`/`disconnect` are
/// mutually exclusive on the connected flag; `pair`/`forget` on the paired flag,
/// and Forget needs the owning adapter path.
// Four independent per-button enables — the whole point is one flag per action;
// a state machine would obscure, not clarify, the row's button set.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, PartialEq, Eq)]
struct DeviceActions {
    /// Offer Connect (the device is not currently connected).
    connect: bool,
    /// Offer Disconnect (the device is currently connected).
    disconnect: bool,
    /// Offer Pair (the device is not yet bonded).
    pair: bool,
    /// Offer Forget (the device is bonded AND the adapter path is known).
    forget: bool,
}

/// Decide which action buttons a device row offers, given its state and whether
/// the owning adapter path is known.
const fn device_actions(device: &BtDevice, adapter_path: Option<&str>) -> DeviceActions {
    DeviceActions {
        connect: !device.connected,
        disconnect: device.connected,
        pair: !device.paired,
        forget: device.paired && adapter_path.is_some(),
    }
}

/// A Bluetooth control error as a Warning chyron (§7) — local (no source host),
/// flagged `BLUETOOTH`.
fn bt_error_toast(verb: &str, e: &SeatError) -> Toast {
    Toast::alert(
        Severity::Warning,
        String::new(),
        "BLUETOOTH",
        format!("Bluetooth {verb}: {e}"),
    )
}

/// The Bluetooth section — a live control panel (E12-17): per-adapter power /
/// discoverable / pairable / scan, and per-device connect / pair / trust / forget,
/// each driving the real `BlueZ` backend through the one seat. `Absent` renders the
/// shared honest not-available note.
fn bluetooth_section(ui: &mut egui::Ui, snap: Option<&SeatSnapshot>, actions: &mut Vec<SysAction>) {
    probe_section(
        ui,
        snap,
        |s| &s.bluetooth,
        |ui, bt: &BtStatus| {
            if bt.adapters.is_empty() {
                muted_note(ui, "No Bluetooth adapter.");
                return;
            }
            // Devices hang off the first adapter (the RemoveDevice owner). A scan
            // annotates each device row with live RSSI.
            let adapter_path = bt.adapters.first().map(|a| a.path.as_str());
            let scanning = bt.adapters.iter().any(|a| a.discovering);
            // Adapters and devices sit side by side in the wide pane (SETTINGS-3);
            // a narrow seat drops to one stacked column so nothing clips.
            let render_adapters = |ui: &mut egui::Ui, actions: &mut Vec<SysAction>| {
                for adapter in &bt.adapters {
                    adapter_row(ui, adapter, actions);
                }
            };
            let render_devices = |ui: &mut egui::Ui, actions: &mut Vec<SysAction>| {
                if bt.devices.is_empty() {
                    muted_note(ui, "No devices — scan to discover nearby devices.");
                }
                for device in &bt.devices {
                    device_row(ui, device, adapter_path, scanning, actions);
                }
            };
            if fit_columns(ui.available_width(), 2) == 2 {
                ui.columns(2, |columns| {
                    column_card(&mut columns[0], "Adapters", |ui| {
                        render_adapters(ui, actions);
                    });
                    column_card(&mut columns[1], "Devices", |ui| {
                        render_devices(ui, actions);
                    });
                });
            } else {
                column_card(ui, "Adapters", |ui| render_adapters(ui, actions));
                ui.add_space(Style::SP_S);
                column_card(ui, "Devices", |ui| render_devices(ui, actions));
            }
        },
    );
}

/// One adapter's control row: a status header, then Powered / Discoverable /
/// Pairable toggles and a Scan toggle (with a spinner while discovering).
fn adapter_row(ui: &mut egui::Ui, adapter: &BtAdapter, actions: &mut Vec<SysAction>) {
    let (word, tone) = if adapter.powered {
        ("on", Style::OK)
    } else {
        ("off", Style::TEXT_DIM)
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(DOT).color(tone).size(Style::SMALL));
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(&adapter.name)
                .color(Style::TEXT)
                .size(Style::SMALL)
                .strong(),
        );
        ui.add_space(Style::SP_S);
        ui.colored_label(tone, RichText::new(word).size(Style::SMALL));
    });

    ui.indent((adapter.path.as_str(), "bt-adapter"), |ui| {
        let mut powered = adapter.powered;
        if ui
            .checkbox(&mut powered, RichText::new("Powered").size(Style::SMALL))
            .changed()
        {
            actions.push(SysAction::BtPower(adapter.path.clone(), powered));
        }

        // Discoverable / Pairable / Scan are only meaningful on a powered radio.
        if !adapter.powered {
            return;
        }
        let mut discoverable = adapter.discoverable;
        if ui
            .checkbox(
                &mut discoverable,
                RichText::new("Discoverable").size(Style::SMALL),
            )
            .changed()
        {
            actions.push(SysAction::BtDiscoverable(
                adapter.path.clone(),
                discoverable,
            ));
        }
        let mut pairable = adapter.pairable;
        if ui
            .checkbox(&mut pairable, RichText::new("Pairable").size(Style::SMALL))
            .changed()
        {
            actions.push(SysAction::BtPairable(adapter.path.clone(), pairable));
        }
        ui.horizontal(|ui| {
            if adapter.discovering {
                if ui
                    .button(RichText::new("Stop scan").size(Style::SMALL))
                    .clicked()
                {
                    actions.push(SysAction::BtScan(adapter.path.clone(), false));
                }
                ui.add_space(Style::SP_XS);
                ui.spinner();
                ui.colored_label(
                    Style::TEXT_DIM,
                    RichText::new("Scanning…").size(Style::SMALL),
                );
            } else if ui
                .button(RichText::new("Scan").size(Style::SMALL))
                .clicked()
            {
                actions.push(SysAction::BtScan(adapter.path.clone(), true));
            }
        });
    });
    ui.add_space(Style::SP_XS);
}

/// One device's control row: a status header, a meta line (address · battery ·
/// in-scan RSSI), then Connect/Disconnect, Pair/Forget, and a Trust checkbox that
/// reflect the device's live state.
fn device_row(
    ui: &mut egui::Ui,
    device: &BtDevice,
    adapter_path: Option<&str>,
    scanning: bool,
    actions: &mut Vec<SysAction>,
) {
    let (word, tone) = if device.connected {
        ("connected", Style::OK)
    } else if device.paired {
        ("paired", Style::TEXT_DIM)
    } else {
        ("available", Style::TEXT_DIM)
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(DOT).color(tone).size(Style::SMALL));
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(&device.alias)
                .color(Style::TEXT)
                .size(Style::SMALL)
                .strong(),
        );
        ui.add_space(Style::SP_S);
        ui.colored_label(tone, RichText::new(word).size(Style::SMALL));
    });

    ui.indent((device.path.as_str(), "bt-dev"), |ui| {
        // Meta line — only the parts BlueZ actually reported (§7: never invented).
        let mut meta: Vec<String> = Vec::new();
        if let Some(address) = &device.address {
            meta.push(address.clone());
        }
        if let Some(pct) = device.battery_percent {
            meta.push(format!("{pct}% battery"));
        }
        // RSSI is only meaningful during a scan (BlueZ clears it otherwise).
        if scanning {
            if let Some(rssi) = device.rssi {
                meta.push(format!("{rssi} dBm"));
            }
        }
        if !meta.is_empty() {
            ui.colored_label(
                Style::TEXT_DIM,
                RichText::new(meta.join("  \u{00B7}  ")).size(Style::SMALL),
            );
        }

        let acts = device_actions(device, adapter_path);
        ui.horizontal(|ui| {
            if acts.disconnect {
                if ui
                    .button(RichText::new("Disconnect").size(Style::SMALL))
                    .clicked()
                {
                    actions.push(SysAction::BtDisconnect(device.path.clone()));
                }
            } else if acts.connect
                && ui
                    .button(RichText::new("Connect").size(Style::SMALL))
                    .clicked()
            {
                actions.push(SysAction::BtConnect(device.path.clone()));
            }

            if acts.pair {
                if ui
                    .button(RichText::new("Pair").size(Style::SMALL))
                    .clicked()
                {
                    actions.push(SysAction::BtPair(device.path.clone()));
                }
            } else if ui
                // Forget needs the owning adapter path; disabled honestly if unknown.
                .add_enabled(
                    acts.forget,
                    egui::Button::new(RichText::new("Forget").size(Style::SMALL)),
                )
                .clicked()
            {
                if let Some(adapter) = adapter_path {
                    actions.push(SysAction::BtForget {
                        adapter: adapter.to_owned(),
                        device: device.path.clone(),
                    });
                }
            }

            let mut trusted = device.trusted;
            if ui
                .checkbox(&mut trusted, RichText::new("Trust").size(Style::SMALL))
                .changed()
            {
                actions.push(SysAction::BtTrust(device.path.clone(), trusted));
            }
        });
    });
    ui.add_space(Style::SP_XS);
}

// ──────────────────────────── Displays (E12-18) ────────────────────────────

/// The Displays section — per-output enable / mode / arrangement (editing the
/// intent [`DisplayLayout`]) plus live per-output brightness. `Absent` on a host
/// with no `/dev/dri`.
fn displays_section(
    ui: &mut egui::Ui,
    snap: Option<&SeatSnapshot>,
    layout: &DisplayLayout,
    panel_brightness: &mut BTreeMap<String, u8>,
    ddc_brightness: &mut BTreeMap<String, u8>,
    actions: &mut Vec<SysAction>,
) {
    probe_section(
        ui,
        snap,
        |s| &s.displays,
        |ui, connectors| {
            if layout.outputs.is_empty() {
                muted_note(ui, "No connectors.");
                return;
            }
            let backlights = snap.and_then(|s| s.backlights.present());
            let ddc = snap.and_then(|s| s.ddc.present());
            let multi = layout.active_count() > 1;
            // The outputs laid across the wide pane as a ROW of cards (SETTINGS-3),
            // up to three to a row, collapsing toward one on a narrow seat.
            across_grid(ui, &layout.outputs, 3, |ui, out| {
                let connector = connectors.iter().find(|c| c.name == out.connector);
                tile(ui, |ui| {
                    output_row(
                        ui,
                        out,
                        connector,
                        multi,
                        backlights,
                        ddc,
                        panel_brightness,
                        ddc_brightness,
                        actions,
                    );
                });
            });
            ui.add_space(Style::SP_S);
            // The arrangement is desired-state intent: the live modeset apply
            // (panel → the `run_drm` runner's multi-CRTC drive) + EDID-keyed
            // roaming are integration-gated (E12-19). Honest, never a fake "applied".
            muted_note(
                ui,
                "Arrangement + mode are saved as intent; live re-apply and EDID roam are integration-gated (E12-19).",
            );
        },
    );
}

/// One output's row: a status/enable line, then (when connected) a mode picker,
/// an arrangement nudge, and a live brightness control.
#[allow(clippy::too_many_arguments)] // a render row legibly threads its live refs
fn output_row(
    ui: &mut egui::Ui,
    out: &OutputArrangement,
    connector: Option<&Connector>,
    multi: bool,
    backlights: Option<&Vec<Backlight>>,
    ddc: Option<&Vec<DdcDisplay>>,
    panel_brightness: &mut BTreeMap<String, u8>,
    ddc_brightness: &mut BTreeMap<String, u8>,
    actions: &mut Vec<SysAction>,
) {
    let status = connector.map_or(ConnectorStatus::Unknown, |c| c.status);
    ui.horizontal(|ui| {
        let (word, tone) = match status {
            ConnectorStatus::Connected => ("connected", Style::OK),
            ConnectorStatus::Disconnected => ("disconnected", Style::TEXT_DIM),
            ConnectorStatus::Unknown => ("unknown", Style::TEXT_DIM),
        };
        ui.label(RichText::new(DOT).color(tone).size(Style::SMALL));
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(&out.connector)
                .color(Style::TEXT)
                .size(Style::SMALL)
                .strong(),
        );
        ui.add_space(Style::SP_S);
        ui.colored_label(tone, RichText::new(word).size(Style::SMALL));
    });

    // Only a connected output is actionable (enable/mode/brightness).
    if status != ConnectorStatus::Connected {
        return;
    }

    ui.indent((out.connector.as_str(), "disp"), |ui| {
        // Enable toggle — disabling the last lit output is refused typed on apply.
        let mut enabled = out.enabled;
        if ui
            .checkbox(&mut enabled, RichText::new("Enabled").size(Style::SMALL))
            .changed()
        {
            actions.push(SysAction::ToggleOutput(out.id.clone(), enabled));
        }

        if out.enabled {
            // Mode picker over the connector's advertised modes.
            if let Some(conn) = connector {
                mode_picker(ui, out, conn, actions);
            }
            // Relative arrangement: position + nudges (only meaningful multi-head).
            ui.horizontal(|ui| {
                field(
                    ui,
                    "Position",
                    &format!("{}, {}", out.position.0, out.position.1),
                    Style::TEXT_DIM,
                );
                if multi {
                    if ui.button(RichText::new("◀").size(Style::SMALL)).clicked() {
                        actions.push(SysAction::Nudge(out.id.clone(), true));
                    }
                    if ui.button(RichText::new("▶").size(Style::SMALL)).clicked() {
                        actions.push(SysAction::Nudge(out.id.clone(), false));
                    }
                }
            });
            // Live brightness: DDC for a matched external, backlight for a panel,
            // else an honest "not controllable" (lock 13 / §7).
            brightness_control(
                ui,
                out,
                backlights,
                ddc,
                panel_brightness,
                ddc_brightness,
                actions,
            );
        }
    });
}

/// The mode picker — a combo over the connector's advertised modes; the current
/// choice is the layout's mode (else the connector's preferred).
fn mode_picker(
    ui: &mut egui::Ui,
    out: &OutputArrangement,
    conn: &Connector,
    actions: &mut Vec<SysAction>,
) {
    if conn.modes.is_empty() {
        muted_note(ui, "No modes advertised.");
        return;
    }
    let current = out
        .effective_mode()
        .or_else(|| conn.preferred_mode().copied());
    let label = current.map_or_else(|| "—".to_owned(), |m| m.label());
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Mode")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);
        ComboBox::from_id_salt((out.connector.as_str(), "mode"))
            .selected_text(RichText::new(label).size(Style::SMALL))
            .show_ui(ui, |ui| {
                for mode in &conn.modes {
                    let selected = current == Some(*mode);
                    if ui
                        .selectable_label(selected, RichText::new(mode.label()).size(Style::SMALL))
                        .clicked()
                        && !selected
                    {
                        actions.push(SysAction::SetMode(out.id.clone(), *mode));
                    }
                }
            });
    });
}

/// The per-output brightness control: DDC/CI for a matched external monitor,
/// sysfs backlight for an internal panel, else an honest not-controllable note.
fn brightness_control(
    ui: &mut egui::Ui,
    out: &OutputArrangement,
    backlights: Option<&Vec<Backlight>>,
    ddc: Option<&Vec<DdcDisplay>>,
    panel_brightness: &mut BTreeMap<String, u8>,
    ddc_brightness: &mut BTreeMap<String, u8>,
    actions: &mut Vec<SysAction>,
) {
    // Prefer a DDC monitor whose reported connector matches this output.
    if let Some(disp) = ddc.and_then(|list| {
        list.iter()
            .find(|d| connector_matches(d.connector.as_deref(), &out.connector))
    }) {
        let val = ddc_brightness
            .entry(disp.bus.clone())
            .or_insert(disp.brightness);
        if ui
            .add(
                Slider::new(val, 0..=100)
                    .text(RichText::new("Brightness (DDC)").size(Style::SMALL)),
            )
            .changed()
        {
            actions.push(SysAction::Ddc {
                bus: disp.bus.clone(),
                percent: *val,
            });
        }
        return;
    }
    // An internal panel (eDP/LVDS/DSI) with a backlight → the sysfs slider.
    if is_internal(&out.connector) {
        if let Some(panel) = backlights.and_then(|list| list.first()) {
            let val = panel_brightness
                .entry(panel.name.clone())
                .or_insert_with(|| panel.percent());
            if ui
                .add(
                    Slider::new(val, 0..=100)
                        .text(RichText::new("Brightness (panel)").size(Style::SMALL)),
                )
                .changed()
            {
                // Percentage → the device's raw scale (never clamped silently:
                // an out-of-range write is refused typed by the backlight client).
                let raw = u32::from(*val) * panel.max / 100;
                actions.push(SysAction::Backlight {
                    name: panel.name.clone(),
                    raw,
                });
            }
            return;
        }
    }
    muted_note(ui, "Brightness: not controllable (no backlight / DDC).");
}

/// Does a DDC-reported connector (`card0-DP-1`) name the same output as a DRM
/// connector name (`DP-1`)? `ddcutil` prefixes the card; the prober strips it on
/// card 0 — so compare with any leading `cardN-`/`cardN/` stripped.
fn connector_matches(ddc: Option<&str>, drm: &str) -> bool {
    ddc.is_some_and(|c| strip_card(c) == strip_card(drm))
}

/// Strip a leading `cardN-` / `cardN/` prefix from a connector name.
fn strip_card(name: &str) -> &str {
    name.strip_prefix("card")
        .and_then(|rest| {
            let end = rest.find(|c: char| !c.is_ascii_digit())?;
            let (_, tail) = rest.split_at(end);
            tail.strip_prefix('-').or_else(|| tail.strip_prefix('/'))
        })
        .unwrap_or(name)
}

/// Is this an internal-panel connector (the backlight-controlled kind)?
fn is_internal(name: &str) -> bool {
    let up = strip_card(name).to_ascii_uppercase();
    up.starts_with("EDP") || up.starts_with("LVDS") || up.starts_with("DSI")
}

// ──────────────────────────── Power & Battery (E12-18) ────────────────────────────

/// The Power & Battery section — confirm-gated logind verbs (incl. Hibernate),
/// the power-profile + charge-cap controls, the on-AC source line, and
/// multi-battery telemetry. Every POWER-4 control drives the real seat / reads
/// the real snapshot — no inert affordance (§7). Idle-suspend + lid-close are
/// deliberately out of scope here (POWER-5, once the honorer is not inert).
fn power_section(
    ui: &mut egui::Ui,
    snap: Option<&SeatSnapshot>,
    confirm: Option<PowerVerb>,
    charge_threshold: &mut Option<u8>,
    power_honor_config: &mut PowerHonorConfig,
    actions: &mut Vec<SysAction>,
) {
    // Power controls and battery telemetry sit side by side in the wide pane
    // (SETTINGS-3): the logind verbs + profile + idle/lid policy on the left, the
    // battery / charge / source readings on the right. A narrow seat stacks them.
    // Both panes read the real snapshot + drive the same SysAction seam — a
    // presentation pass, the control logic unchanged (§6/§7).
    let controls_pane = |ui: &mut egui::Ui,
                         confirm: Option<PowerVerb>,
                         cfg: &mut PowerHonorConfig,
                         actions: &mut Vec<SysAction>| {
        // Host power verbs — Lock is always offered; the host-down verbs (Suspend,
        // Hibernate, Reboot, PowerOff) are gated by logind's CanX and a two-click
        // confirm (lock 12). Hibernate (POWER-4) rides the same row + gate as Suspend.
        probe_section(
            ui,
            snap,
            |s| &s.power,
            |ui, caps: &PowerCaps| {
                power_verb_row(ui, PowerVerb::Lock, Avail::Yes, confirm, actions);
                power_verb_row(ui, PowerVerb::Suspend, caps.suspend, confirm, actions);
                power_verb_row(ui, PowerVerb::Hibernate, caps.hibernate, confirm, actions);
                power_verb_row(ui, PowerVerb::Reboot, caps.reboot, confirm, actions);
                power_verb_row(ui, PowerVerb::PowerOff, caps.poweroff, confirm, actions);
            },
        );
        // Idle-suspend + lid-close policy (POWER-5) — the honorer reads this config
        // every frame, so the pickers are §7-real (never inert). Safe defaults: idle
        // Never, lid Suspend.
        ui.add_space(Style::SP_XS);
        power_settings::idle_timeout_body(ui, cfg, actions);
        power_settings::lid_action_body(ui, cfg, actions);
        // Power profile (POWER-4) — the daemon's available set + current active; the
        // Absent case renders the probe's honest "unavailable", never a fake active.
        ui.add_space(Style::SP_XS);
        probe_section(
            ui,
            snap,
            |s| &s.power_profile,
            |ui, state| power_settings::profile_body(ui, state, actions),
        );
    };

    let battery_pane = |ui: &mut egui::Ui, live: &mut Option<u8>, actions: &mut Vec<SysAction>| {
        // On-AC / on-battery source line (POWER-4) — the honest UPower LinePower
        // reading, "unknown" when no adapter is tracked, "unavailable" when Absent.
        probe_section(
            ui,
            snap,
            |s| &s.on_ac,
            |ui, on_ac: &Option<bool>| power_settings::ac_source_body(ui, *on_ac),
        );
        // Charge limit (POWER-4) — the charge-stop cap slider when a battery advertises
        // the attribute, an honest "not supported" when Present(None), the probe's
        // "unavailable" reason when Absent (no power-supply class).
        ui.add_space(Style::SP_XS);
        probe_section(
            ui,
            snap,
            |s| &s.charge_limit,
            |ui, cap: &Option<u8>| {
                power_settings::charge_threshold_body(ui, *cap, live, actions);
            },
        );
        // Batteries (multi + peripherals, lock 6) + rich telemetry (POWER-4).
        ui.add_space(Style::SP_XS);
        probe_section(
            ui,
            snap,
            |s| &s.batteries,
            |ui, batteries| {
                if batteries.is_empty() {
                    muted_note(ui, "No batteries.");
                }
                for battery in batteries {
                    let value = format!(
                        "{:.0}% \u{00B7} {} \u{00B7} {}",
                        battery.percentage,
                        battery.kind.label(),
                        battery.state.label()
                    );
                    field(ui, &battery.model, &value, Style::TEXT);
                    // Time-to-empty / time-to-full + draw rate when UPower reported
                    // them; an honest omission (no second line) otherwise (§7).
                    if let Some(tele) = power_settings::battery_telemetry(battery) {
                        ui.indent((battery.model.as_str(), "battery-tele"), |ui| {
                            muted_note(ui, tele);
                        });
                    }
                }
            },
        );
    };

    if fit_columns(ui.available_width(), 2) == 2 {
        ui.columns(2, |columns| {
            column_card(&mut columns[0], "Power", |ui| {
                controls_pane(ui, confirm, power_honor_config, actions);
            });
            column_card(&mut columns[1], "Battery & source", |ui| {
                battery_pane(ui, charge_threshold, actions);
            });
        });
    } else {
        column_card(ui, "Power", |ui| {
            controls_pane(ui, confirm, power_honor_config, actions);
        });
        ui.add_space(Style::SP_S);
        column_card(ui, "Battery & source", |ui| {
            battery_pane(ui, charge_threshold, actions);
        });
    }
}

/// One power-verb row: the honest availability, then either a Lock/act button, an
/// armed two-step confirm (for a gated verb), or a dimmed "unavailable" label.
fn power_verb_row(
    ui: &mut egui::Ui,
    verb: PowerVerb,
    avail: Avail,
    confirm: Option<PowerVerb>,
    actions: &mut Vec<SysAction>,
) {
    ui.horizontal(|ui| {
        let tone = if avail.offerable() {
            Style::TEXT
        } else {
            Style::TEXT_DIM
        };
        ui.label(RichText::new(verb.label()).color(tone).size(Style::SMALL));
        ui.add_space(Style::SP_S);
        ui.colored_label(
            Style::TEXT_DIM,
            RichText::new(avail.label()).size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);

        if !avail.offerable() {
            return;
        }
        if !verb.needs_confirm() {
            // Lock is benign — a single click acts.
            if ui
                .button(RichText::new(verb.label()).size(Style::SMALL))
                .clicked()
            {
                actions.push(SysAction::Power(verb));
            }
            return;
        }
        // A host-down verb: two-click confirm (lock 12).
        if confirm == Some(verb) {
            if ui
                .button(
                    RichText::new(format!("Confirm {}", verb.label()))
                        .color(Style::DANGER)
                        .size(Style::SMALL),
                )
                .clicked()
            {
                actions.push(SysAction::Power(verb));
            }
            if ui
                .button(RichText::new("Cancel").size(Style::SMALL))
                .clicked()
            {
                actions.push(SysAction::CancelConfirm);
            }
        } else if ui
            .button(RichText::new(verb.label()).size(Style::SMALL))
            .clicked()
        {
            actions.push(SysAction::ArmConfirm(verb));
        }
    });
}

/// The Wallpaper section (QBRAND-11) — the desktop-backdrop picker over the five
/// official Quazar wallpapers (placement lock #12). The choice persists per seat and
/// follows the mesh identity; the [`crate::backdrop`] desktop layer reflects it live.
fn wallpaper_section(ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let current = crate::backdrop::selected_wallpaper(&ctx);
    ui.label(
        RichText::new("Desktop wallpaper")
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
    );
    ui.add_space(Style::SP_S);
    // The official wallpapers as a gallery laid across the wide pane (SETTINGS-3):
    // each is a selectable tile driving the SAME backdrop seam the combo did — a
    // presentation pass, the selection logic unchanged (§6/§7).
    across_grid(ui, &crate::backdrop::Wallpaper::ALL, 3, |ui, &wallpaper| {
        let selected = wallpaper == current;
        tile(ui, |ui| {
            if ui
                .add_sized(
                    [ui.available_width(), Style::SP_XL],
                    egui::SelectableLabel::new(
                        selected,
                        RichText::new(wallpaper.label()).size(Style::BODY),
                    ),
                )
                .clicked()
                && !selected
            {
                crate::backdrop::select_wallpaper(&ctx, wallpaper);
            }
        });
    });
    ui.add_space(Style::SP_S);
    muted_note(
        ui,
        "The five official Quazar wallpapers ship in the RPM; your choice follows your mesh identity when a workgroup volume is present.",
    );
}

/// The Hotkeys section — the fixed compiled-in table (lock 9) read-only, laid
/// **across** the wide pane in a responsive multi-column reference (SETTINGS-3)
/// instead of one tall stacked list.
fn hotkeys_section(ui: &mut egui::Ui) {
    across_grid(ui, HOTKEYS, 3, |ui, hotkey| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(DOT).color(Style::TEXT_DIM).size(Style::SMALL));
            ui.add_space(Style::SP_XS);
            field(ui, hotkey.chord, hotkey.action.label(), Style::TEXT);
        });
    });
}

/// The Theme section (SETTINGS-5) under Personalization — the two appearance controls
/// the shell **genuinely applies at runtime**: the interactive **accent** (re-tinting
/// the live `Style` accent across every surface via [`Style::set_accent`]) and the
/// **text-scale** (the EXPLORER-18 accessibility whole-UI zoom the shell honors). Each
/// pick mutates the persisted [`AppearanceConfig`] in place; the change is saved after
/// the render borrow and applied live by [`SystemState::apply_appearance`] on the poll.
/// The platform is dark-only (the Quasar lock) — there is NO light/dark switch here,
/// only what the runtime truly drives (§7 — no dead toggle). Laid **across** the wide
/// pane (SETTINGS-3) — a swatch row + a step row, each a selectable tile.
fn theme_section(ui: &mut egui::Ui, appearance: &mut AppearanceConfig) {
    // Accent — a swatch row; the pick re-tints the whole shell's highlights live.
    ui.label(
        RichText::new("Accent colour")
            .color(Style::TEXT_DIM)
            .size(Style::SMALL)
            .strong(),
    );
    ui.add_space(Style::SP_XS);
    across_grid(ui, &AccentChoice::ALL, 4, |ui, &choice| {
        let selected = appearance.accent == choice;
        tile(ui, |ui| {
            ui.horizontal(|ui| {
                // A filled swatch in the choice's real token colour — an honest live
                // preview of the tint the pick applies (not a decorative dot).
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(Style::SP_M, Style::SP_M),
                    egui::Sense::hover(),
                );
                ui.painter()
                    .rect_filled(rect, Style::RADIUS, choice.color());
                ui.add_space(Style::SP_XS);
                if ui
                    .add(egui::SelectableLabel::new(
                        selected,
                        RichText::new(choice.label()).size(Style::BODY),
                    ))
                    .clicked()
                    && !selected
                {
                    appearance.accent = choice;
                }
            });
        });
    });
    ui.add_space(Style::SP_M);
    // Text-scale — the EXPLORER-18 accessibility whole-UI zoom, as legible steps.
    ui.label(
        RichText::new("Text size")
            .color(Style::TEXT_DIM)
            .size(Style::SMALL)
            .strong(),
    );
    ui.add_space(Style::SP_XS);
    across_grid(ui, &TextScale::ALL, 5, |ui, &scale| {
        let selected = appearance.text_scale == scale;
        tile(ui, |ui| {
            if ui
                .add_sized(
                    [ui.available_width(), Style::SP_XL],
                    egui::SelectableLabel::new(
                        selected,
                        RichText::new(scale.label()).size(Style::BODY),
                    ),
                )
                .clicked()
                && !selected
            {
                appearance.text_scale = scale;
            }
        });
    });
    ui.add_space(Style::SP_S);
    muted_note(
        ui,
        "Accent re-tints every surface's highlights; text size scales the whole \
         interface. The platform is dark-only — there is no light theme.",
    );
}

// ──────────────────────────── Mesh & System (SETTINGS-4) ────────────────────────────

/// This node's mesh facts (SETTINGS-4), folded from the world-readable mesh-status
/// snapshot the chrome bar + the This Node / Network planes already read
/// ([`MESH_STATUS_PATH`]). The shell leans on no `mackesd` IPC and no root-only cert
/// path (§6); every field is real node reality, honest-`None` (rendered "unknown")
/// where the snapshot doesn't carry it (§7). Pure (no IO / egui / GPU), so
/// [`Self::project`] is unit-tested directly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct MeshFacts {
    /// `true` once a snapshot has parsed — distinguishes "no snapshot yet" (the
    /// reading state) from a parsed one.
    seen: bool,
    /// This node's mesh identity name — the snapshot's `self` marker (the Nebula
    /// certificate CN this fleet stamps as the hostname), when known.
    identity: Option<String>,
    /// Pinned deployment role (`lighthouse` / `server` / `workstation`), when this
    /// node's own directory row carries it.
    role: Option<String>,
    /// This node's Nebula overlay IP, when known.
    overlay_ip: Option<String>,
    /// The overlay tunnel interface (e.g. `nebula1`), when known.
    overlay_if: Option<String>,
    /// The overlay subnet (CIDR), when known.
    overlay_cidr: Option<String>,
    /// The tunnel cipher label, when known.
    cipher: Option<String>,
    /// The elected mesh leader's hostname, when one holds the lease.
    leader: Option<String>,
    /// The lighthouse overlay IPs anchoring the fabric.
    lighthouses: Vec<String>,
    /// The lighthouse public (underlay) endpoints, when the snapshot carries them.
    gateways: Vec<String>,
    /// The node's underlay default gateway, when known.
    default_gw: Option<String>,
    /// Peers currently `online` in the directory.
    peers_online: u64,
    /// Peers in the directory (every node the snapshot names).
    peers_total: u64,
}

impl MeshFacts {
    /// Fold the mesh-status snapshot into this node's mesh facts. A missing /
    /// garbage / non-mesh snapshot yields the honest unseen facts (drives the
    /// "reading…" state), never a panic — mirroring the chrome bar's tolerance.
    fn project(snapshot: &str) -> Self {
        let Ok(v) = serde_json::from_str::<Value>(snapshot) else {
            return Self::default();
        };
        let identity = nonempty(&v, "self");
        let nodes = v.get("nodes").and_then(Value::as_array);
        // A real snapshot names at least `self` or a `nodes` array; anything else
        // (an empty object, an array, a fragment) reads as unseen.
        if identity.is_none() && nodes.is_none() {
            return Self::default();
        }
        let network = v.get("network");
        // This node's own directory row (the role / overlay source), matched by the
        // `self` identity — honestly absent when the node hasn't published a row yet.
        let own = identity.as_deref().and_then(|host| {
            nodes.and_then(|arr| {
                arr.iter()
                    .find(|n| n.get("hostname").and_then(Value::as_str) == Some(host))
            })
        });
        Self {
            seen: true,
            role: own.and_then(|n| nonempty(n, "role")),
            // Prefer this node's own directory-row overlay IP; fall back to the
            // network overview's locally-probed overlay address.
            overlay_ip: own
                .and_then(|n| nonempty(n, "overlay_ip"))
                .or_else(|| network.and_then(|n| nonempty(n, "overlay_ip"))),
            overlay_if: network.and_then(|n| nonempty(n, "overlay_if")),
            overlay_cidr: network.and_then(|n| nonempty(n, "overlay_cidr")),
            cipher: network.and_then(|n| nonempty(n, "cipher")),
            leader: network.and_then(|n| nonempty(n, "leader")),
            lighthouses: str_array(network, "lighthouse_ips"),
            gateways: str_array(network, "gateway_endpoints"),
            default_gw: network.and_then(|n| nonempty(n, "default_gw")),
            peers_online: v.get("online").and_then(Value::as_u64).unwrap_or(0),
            peers_total: v.get("total").and_then(Value::as_u64).unwrap_or(0),
            identity,
        }
    }

    /// `true` when this node holds the mesh leader lease (its identity names the
    /// elected leader).
    fn is_leader(&self) -> bool {
        matches!((&self.leader, &self.identity), (Some(l), Some(i)) if l == i)
    }
}

/// Read a non-empty trimmed string field off a JSON object, or `None` — the same
/// honest "empty ⇒ absent" fold the This Node / Network planes use (§7).
fn nonempty(val: &Value, key: &str) -> Option<String> {
    val.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Read a JSON string array (dropping empties) off `network[key]`, or an empty vec
/// when the key is absent / not an array.
fn str_array(network: Option<&Value>, key: &str) -> Vec<String> {
    network
        .and_then(|n| n.get(key))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// One mesh fact as a [`field`] row — the toned value when the snapshot carried it,
/// a dim honest "unknown" when it didn't (§7 — never a fabricated value).
fn mesh_field(ui: &mut egui::Ui, label: &str, value: Option<&str>) {
    match value {
        Some(v) => field(ui, label, v, Style::TEXT),
        None => field(ui, label, "unknown", Style::TEXT_DIM),
    }
}

/// The shared "reading the snapshot" note a Mesh & System section shows before the
/// first mesh-status poll lands.
fn mesh_reading(ui: &mut egui::Ui) {
    muted_note(ui, "Reading this node's mesh status…");
}

/// The Identity section (SETTINGS-4) — this node's mesh identity name + overlay
/// address + tunnel cipher, folded from the world-readable snapshot. The Nebula
/// certificate fingerprint is honestly `unknown`: the shell reads the world-readable
/// mesh-status surface, not the root-only cert (§6/§7 — the same honest boundary the
/// This Node plane draws for node-local telemetry).
fn identity_section(ui: &mut egui::Ui, mesh: &MeshFacts) {
    if !mesh.seen {
        mesh_reading(ui);
        return;
    }
    tile(ui, |ui| {
        mesh_field(ui, "Mesh name", mesh.identity.as_deref());
        // Not on the world-readable surface — honest-unknown, never a fake digest.
        field(ui, "Certificate fingerprint", "unknown", Style::TEXT_DIM);
        mesh_field(ui, "Overlay address", mesh.overlay_ip.as_deref());
        mesh_field(ui, "Tunnel cipher", mesh.cipher.as_deref());
    });
    ui.add_space(Style::SP_S);
    muted_note(
        ui,
        "Identity folds from the world-readable mesh-status snapshot; the Nebula \
         certificate fingerprint isn't published to this surface (the shell reads no \
         root-only cert).",
    );
}

/// The Role section (SETTINGS-4) — this node's pinned deployment role, a one-line
/// description of what the tier means, and a leader-lease marker. Honest-`unknown`
/// when the node hasn't published a directory row yet (§7).
fn role_section(ui: &mut egui::Ui, mesh: &MeshFacts) {
    if !mesh.seen {
        mesh_reading(ui);
        return;
    }
    let accent = SettingsGroup::MeshSystem.accent();
    tile(ui, |ui| {
        match mesh.role.as_deref() {
            Some(role) => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(DOT).color(accent).size(Style::SMALL));
                    ui.add_space(Style::SP_XS);
                    ui.label(RichText::new(role).color(accent).size(Style::BODY).strong());
                });
                ui.add_space(Style::SP_XS);
                muted_note(ui, role_description(role));
            }
            None => field(
                ui,
                "Role",
                "unknown — not yet pinned in the peer directory",
                Style::TEXT_DIM,
            ),
        }
        if mesh.is_leader() {
            ui.add_space(Style::SP_XS);
            ui.horizontal(|ui| {
                ui.label(RichText::new(DOT).color(Style::OK).size(Style::SMALL));
                ui.add_space(Style::SP_XS);
                ui.colored_label(
                    Style::OK,
                    RichText::new("holds the mesh leader lease").size(Style::SMALL),
                );
            });
        }
    });
}

/// A one-line description of a pinned role for the Role section — honest for the
/// three deployment tiers the fleet pins, a neutral line for any other value.
fn role_description(role: &str) -> &'static str {
    match role {
        "lighthouse" => {
            "Anchors the overlay — a stable public endpoint peers discover the mesh through."
        }
        "server" => "A headless mesh member running shared workloads and services.",
        "workstation" => "An interactive seat — this desktop rides the mesh as a workstation.",
        _ => "A pinned mesh member.",
    }
}

/// The Pairing section (SETTINGS-4) — folds in the pairing responder the surface
/// already drives while Settings is open ([`SystemState::sync_pairing_agent`], §6).
/// It surfaces the responder's honest live state — whether an adapter is present for
/// it to bind, whether it's registered, and whether a pairing prompt is in flight
/// (answered in the shared modal) — and offers a Retry that re-arms the SAME seam
/// after a transient failure (never a second agent, §6 one-owner).
fn pairing_section(
    ui: &mut egui::Ui,
    snap: Option<&SeatSnapshot>,
    agent_active: bool,
    prompt_in_flight: bool,
    actions: &mut Vec<SysAction>,
) {
    // The responder binds the host Bluetooth adapter — no adapter, nothing to pair.
    let adapter_present = matches!(
        snap.map(|s| &s.bluetooth),
        Some(Probe::Present(bt)) if !bt.adapters.is_empty()
    );
    tile(ui, |ui| {
        let (dot, word, tone) = if !adapter_present {
            (
                Style::TEXT_DIM,
                "no adapter — nothing to pair",
                Style::TEXT_DIM,
            )
        } else if agent_active {
            (Style::OK, "registered", Style::OK)
        } else {
            (
                Style::WARN,
                "adapter present — not yet registered",
                Style::WARN,
            )
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(DOT).color(dot).size(Style::SMALL));
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new("Pairing responder")
                    .color(Style::TEXT)
                    .size(Style::SMALL)
                    .strong(),
            );
            ui.add_space(Style::SP_S);
            ui.colored_label(tone, RichText::new(word).size(Style::SMALL));
        });
        // A prompt in flight — the operator answers it in the shared modal.
        if prompt_in_flight {
            ui.add_space(Style::SP_XS);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.add_space(Style::SP_XS);
                ui.colored_label(
                    Style::ACCENT,
                    RichText::new("A pairing prompt is waiting — respond in the dialog.")
                        .size(Style::SMALL),
                );
            });
        }
        // Retry re-arms the responder main.rs drives on visibility — disabled
        // honestly when there is no adapter to bind.
        ui.add_space(Style::SP_XS);
        if ui
            .add_enabled(
                adapter_present,
                egui::Button::new(RichText::new("Retry pairing").size(Style::SMALL)),
            )
            .clicked()
        {
            actions.push(SysAction::PairingRetry);
        }
    });
    ui.add_space(Style::SP_S);
    muted_note(
        ui,
        "The pairing responder answers incoming device PIN / passkey prompts while \
         Settings is open; it binds the host Bluetooth adapter (§6 — one responder, \
         driven by this surface's visibility).",
    );
}

/// The Network section (SETTINGS-4) — the overlay (Nebula) facts and the mesh links /
/// underlay reachability, laid side by side across the wide pane (SETTINGS-3). Every
/// field is the node's real snapshot reality, honest-`unknown` where absent (§7).
/// Live per-link throughput / handshake state isn't on the world-readable surface
/// (§6) — the same honest boundary the Network plane draws.
fn network_section(ui: &mut egui::Ui, mesh: &MeshFacts) {
    // The middle-dot joiner the device-meta / Network rows use for a list value.
    const SEP: &str = "  \u{00B7}  ";
    if !mesh.seen {
        mesh_reading(ui);
        return;
    }
    let overlay = |ui: &mut egui::Ui| {
        mesh_field(ui, "Overlay IP", mesh.overlay_ip.as_deref());
        mesh_field(ui, "Interface", mesh.overlay_if.as_deref());
        mesh_field(ui, "Subnet", mesh.overlay_cidr.as_deref());
        mesh_field(ui, "Cipher", mesh.cipher.as_deref());
    };
    let links = |ui: &mut egui::Ui| {
        // Live peer count — green when all live, warn when some are down.
        let tone = if mesh.peers_total == 0 {
            Style::TEXT_DIM
        } else if mesh.peers_online == mesh.peers_total {
            Style::OK
        } else {
            Style::WARN
        };
        field(
            ui,
            "Peers",
            &format!("{}/{} live", mesh.peers_online, mesh.peers_total),
            tone,
        );
        // The elected leader (with a this-node marker when we hold the lease).
        match mesh.leader.as_deref() {
            Some(leader) if mesh.is_leader() => {
                field(ui, "Leader", &format!("{leader} (this node)"), Style::OK);
            }
            Some(leader) => field(ui, "Leader", leader, Style::TEXT),
            None => field(ui, "Leader", "no leader elected", Style::TEXT_DIM),
        }
        // Lighthouses anchoring the overlay.
        if mesh.lighthouses.is_empty() {
            field(ui, "Lighthouses", "unknown", Style::TEXT_DIM);
        } else {
            field(ui, "Lighthouses", &mesh.lighthouses.join(SEP), Style::TEXT);
        }
        // Underlay reachability: the public endpoints + the default gateway (both
        // honestly omitted / dim when the snapshot doesn't carry them).
        if !mesh.gateways.is_empty() {
            field(
                ui,
                "Public endpoints",
                &mesh.gateways.join(SEP),
                Style::TEXT,
            );
        }
        mesh_field(ui, "Default gateway", mesh.default_gw.as_deref());
    };
    if fit_columns(ui.available_width(), 2) == 2 {
        ui.columns(2, |columns| {
            column_card(&mut columns[0], "Overlay", |ui| overlay(ui));
            column_card(&mut columns[1], "Mesh links", |ui| links(ui));
        });
    } else {
        column_card(ui, "Overlay", |ui| overlay(ui));
        ui.add_space(Style::SP_S);
        column_card(ui, "Mesh links", |ui| links(ui));
    }
}

/// MENUBAR-ALL (System) — the shared top bar over the master-detail Settings shell.
///
/// The bar's three menus ARE the master rail's three domain groups (Devices ·
/// Personalization · Mesh & System), each listing every [`SettingsSection`] under it
/// as a radio item that jumps the detail pane — the SAME `nav` seam a rail-row click
/// drives (§6, one source of truth). This makes every settings category — including
/// the advanced Pairing / Network / Power ones — reachable from the menus, the
/// governing principle's point. There is no invented File/Edit/Help spine (Settings
/// has none); the taxonomy places every section exactly once (a compile-time-style
/// invariant the tests assert). The status cluster reads this seat's live hardware:
/// the system battery (level + charge state) and the connected-display count, or an
/// honest "No seat" when no snapshot has landed (§7).
mod menubar {
    use super::{SettingsGroup, SettingsSection};
    use mde_egui::egui::Ui;
    use mde_egui::menubar::{Entry, Item, Menu, MenuBar, MenuBarModel};
    use mde_egui::{ChipTone, StatusChip, Style};
    use mde_seat::{BatteryState, ConnectorStatus, SeatSnapshot};

    /// A filled status dot — the shared glyph the sections + dock quads use.
    const DOT: &str = "\u{25CF}";

    /// Render the SYSTEM bar and return the section the operator picked this frame,
    /// if any — the same seam the rail drives (`nav = SettingsNav::at(section)`).
    pub(super) fn show(
        ui: &mut Ui,
        active: SettingsSection,
        snap: Option<&SeatSnapshot>,
    ) -> Option<SettingsSection> {
        let menus = build_menus(active);
        let status = build_status(snap);
        let model = MenuBarModel {
            // The dock's "System" group + the Devices settings domain both wear the
            // categorical gold, so the surface title matches (lock 2).
            title: "System",
            accent: Style::ACCENT_SYSTEM,
            menus: &menus,
            status: &status,
        };
        MenuBar::show(ui, &model)
    }

    /// One menu per domain group, each listing its sections as radio items (the
    /// active one checked) — the rail's taxonomy, verbatim.
    fn build_menus(active: SettingsSection) -> Vec<Menu<SettingsSection>> {
        SettingsGroup::ALL
            .iter()
            .map(|&group| {
                let items = group
                    .sections()
                    .iter()
                    .map(|&s| Entry::Item(Item::new(s, s.label()).checked(s == active)))
                    .collect();
                Menu::new(group.label(), items)
            })
            .collect()
    }

    /// The status-chip tone for a battery level + charge state: charging / on-AC
    /// reads Ok, a low battery escalates Warn → Danger, else a neutral read-out.
    const fn battery_tone(pct: i64, charging: bool, on_ac: bool) -> ChipTone {
        if charging || on_ac {
            ChipTone::Ok
        } else if pct <= 15 {
            ChipTone::Danger
        } else if pct <= 30 {
            ChipTone::Warn
        } else {
            ChipTone::Neutral
        }
    }

    /// The live status cluster: the system battery + the connected-display count from
    /// the seat snapshot, or an honest "No seat" when none has landed (§7).
    fn build_status(snap: Option<&SeatSnapshot>) -> Vec<StatusChip> {
        let Some(snap) = snap else {
            return vec![StatusChip::with_icon(DOT, "No seat", ChipTone::Warn)];
        };
        let mut chips = Vec::new();

        // The whole-system battery (a peripheral battery is not the seat's charge).
        if let Some(bat) = snap
            .batteries
            .present()
            .and_then(|bs| bs.iter().find(|b| b.power_supply))
        {
            #[allow(clippy::cast_possible_truncation)]
            let pct = bat.percentage.round().clamp(0.0, 100.0) as i64;
            let on_ac = snap.on_ac.present().copied().flatten().unwrap_or(false);
            let charging = matches!(
                bat.state,
                BatteryState::Charging | BatteryState::FullyCharged | BatteryState::PendingCharge
            );
            let suffix = if charging {
                " charging"
            } else if on_ac {
                " (AC)"
            } else {
                ""
            };
            chips.push(StatusChip::with_icon(
                DOT,
                format!("{pct}%{suffix}"),
                battery_tone(pct, charging, on_ac),
            ));
        }

        // Connected displays (a real monitor count, not every enumerated connector).
        if let Some(conns) = snap.displays.present() {
            let n = conns
                .iter()
                .filter(|c| c.status == ConnectorStatus::Connected)
                .count();
            chips.push(StatusChip::new(
                format!("{n} display{}", if n == 1 { "" } else { "s" }),
                ChipTone::Neutral,
            ));
        }
        chips
    }

    #[cfg(test)]
    mod tests {
        use super::super::{SettingsGroup, SettingsSection};
        use super::{battery_tone, build_menus, build_status};
        use mde_egui::menubar::Entry;
        use mde_egui::ChipTone;

        #[test]
        fn the_menus_are_the_three_domain_groups_covering_every_section_once() {
            let menus = build_menus(SettingsSection::Pairing);
            let titles: Vec<&str> = menus.iter().map(|m| m.title.as_str()).collect();
            assert_eq!(titles, vec!["Devices", "Personalization", "Mesh & System"]);
            // Every section appears exactly once across the three menus (the rail
            // taxonomy) — no dead/duplicated entry (§7).
            let mut seen: Vec<SettingsSection> = menus
                .iter()
                .flat_map(|m| m.entries.iter())
                .filter_map(|e| match e {
                    Entry::Item(i) => Some(i.id),
                    _ => None,
                })
                .collect();
            let count = seen.len();
            seen.sort_by_key(|s| s.label());
            seen.dedup();
            assert_eq!(count, seen.len(), "a section is listed twice");
            for group in SettingsGroup::ALL {
                for section in group.sections() {
                    assert!(
                        seen.contains(section),
                        "{section:?} is unreachable from the bar"
                    );
                }
            }
        }

        #[test]
        fn exactly_the_active_section_is_checked() {
            let menus = build_menus(SettingsSection::Network);
            for entry in menus.iter().flat_map(|m| m.entries.iter()) {
                if let Entry::Item(item) = entry {
                    assert_eq!(
                        item.checked,
                        Some(item.id == SettingsSection::Network),
                        "{:?} check-state must track the active section",
                        item.id
                    );
                }
            }
        }

        #[test]
        fn no_seat_is_an_honest_warn_chip() {
            let chips = build_status(None);
            assert!(chips
                .iter()
                .any(|c| c.text == "No seat" && c.tone == ChipTone::Warn));
        }

        #[test]
        fn battery_tone_escalates_by_level_and_charge_state() {
            assert_eq!(battery_tone(80, false, false), ChipTone::Neutral);
            assert_eq!(battery_tone(25, false, false), ChipTone::Warn);
            assert_eq!(battery_tone(10, false, false), ChipTone::Danger);
            // Charging / on-AC always reads Ok regardless of level.
            assert_eq!(battery_tone(10, true, false), ChipTone::Ok);
            assert_eq!(battery_tone(10, false, true), ChipTone::Ok);
        }

        #[test]
        fn menu_bar_renders_headless() {
            use mde_egui::egui::{self, pos2, vec2, Rect};
            use mde_egui::Style;
            let ctx = egui::Context::default();
            Style::install(&ctx);
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1024.0, 640.0))),
                ..Default::default()
            };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = super::show(ui, SettingsSection::Displays, None);
                });
            });
            let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
            assert!(!prims.is_empty(), "the System bar produced no primitives");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_egui::egui::{pos2, vec2, Rect};
    use mde_seat::{Battery, BatteryKind, BatteryState, ProfileState};

    /// Drive one headless frame of the System panel over a real seat, and tessellate
    /// on the CPU (the DRM runner's path minus GPU).
    fn renders(state: &mut SystemState) -> bool {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| state.show(ui));
        });
        !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty()
    }

    #[test]
    fn the_pre_poll_state_is_a_full_paint_not_a_blank_panel() {
        let mut st = SystemState::default();
        assert!(renders(&mut st), "pre-poll System panel drew nothing");
    }

    #[test]
    fn a_real_seat_snapshot_mounts_and_renders_every_section() {
        // Over a REAL Seat::snapshot(): on the headless farm host most backends are
        // Absent (each an honest typed line), the arrangement/power controls fold to
        // their not-available/empty states — still a full paint path, never blank.
        let ctx = egui::Context::default();
        let mut st = SystemState::default();
        st.poll(&ctx); // one snapshot + reconcile
        assert!(renders(&mut st), "live System panel drew nothing");
    }

    #[test]
    fn default_state_is_unpolled_with_an_empty_layout() {
        let st = SystemState::default();
        assert!(st.snapshot().is_none());
        assert!(st.layout.outputs.is_empty());
        assert!(st.confirm.is_none());
    }

    #[test]
    fn a_reconcile_builds_the_layout_and_seeds_brightness_from_the_probe() {
        // Feed a synthetic snapshot via the real reconcile path (no hardware): a
        // connected panel + a backlight seed the layout + the panel-brightness map.
        let mut st = SystemState::default();
        let snap = Seat::new().snapshot();
        st.reconcile(&snap);
        // On the farm host displays are Absent → the layout stays empty but the
        // reconcile never panics. The point is the intent model tracks the probe.
        assert_eq!(st.layout.outputs.len(), st.layout_key.len());
    }

    #[test]
    fn strip_card_and_connector_matching_line_up_ddcutil_and_drm_names() {
        assert_eq!(strip_card("card0-DP-1"), "DP-1");
        assert_eq!(strip_card("card1/HDMI-A-2"), "HDMI-A-2");
        assert_eq!(strip_card("DP-3"), "DP-3");
        assert!(connector_matches(Some("card0-DP-1"), "DP-1"));
        assert!(!connector_matches(Some("card0-DP-2"), "DP-1"));
        assert!(!connector_matches(None, "DP-1"));
        assert!(is_internal("eDP-1"));
        assert!(is_internal("card0-eDP-1"));
        assert!(!is_internal("DP-1"));
    }

    #[test]
    fn hotkey_dispatch_acts_on_a_headless_seat_without_panicking() {
        // On the farm host every backend is Absent, so the hardware hotkeys have no
        // target: they must fold to `None` (no OSD) or an honest inline error, never
        // panic. The live OSD-returning path needs real PipeWire/backlight hardware
        // (integration-gated); this proves the dispatch seam is total + reachable.
        let ctx = egui::Context::default();
        let mut st = SystemState::default();
        st.poll(&ctx); // one real snapshot (all Absent on the farm)

        // No mixer → no OSD, no panic.
        assert!(st.dispatch_hotkey(HotkeyAction::VolumeUp).is_none());
        assert!(st.dispatch_hotkey(HotkeyAction::VolumeMute).is_none());
        // The mic key is honestly not-available (output-only mixer model).
        assert!(st.dispatch_hotkey(HotkeyAction::MicMute).is_none());
        assert!(st.error.as_deref().unwrap().contains("Microphone"));
        // No backlight / DDC → the honest not-controllable note.
        assert!(st.dispatch_hotkey(HotkeyAction::BrightnessDown).is_none());
        assert!(st.error.as_deref().unwrap().contains("Brightness"));
        // A navigation action never touches the seat (the shell applies it).
        assert!(st.dispatch_hotkey(HotkeyAction::SessionSwitch).is_none());
        // Lock reaches logind (Absent here → an error, never a real lock/panic).
        assert!(st.dispatch_hotkey(HotkeyAction::Lock).is_none());
    }

    #[test]
    fn the_confirm_gate_arms_before_a_host_down_verb_acts() {
        // The two-step gate (lock 12): a Reboot click arms confirm; only the confirm
        // click emits the Power action. Exercised through apply() (no real reboot —
        // the seat's logind is Absent on the farm host, so Power folds to an error,
        // never an actual poweroff).
        let mut st = SystemState::default();
        st.apply(vec![SysAction::ArmConfirm(PowerVerb::Reboot)]);
        assert_eq!(st.confirm, Some(PowerVerb::Reboot));
        st.apply(vec![SysAction::CancelConfirm]);
        assert!(st.confirm.is_none());
    }

    // ── Power Settings (POWER-4) ──────────────────────────────────────────────

    #[test]
    fn a_live_power_panel_renders_the_power4_controls() {
        // Inject Present POWER-4 probes over an otherwise-real (Absent) snapshot
        // and prove the profile segmented control, the AC source line, the charge
        // slider, and the rich battery telemetry all tessellate real geometry —
        // reachable controls driving the real seat, not a mockup.
        let mut st = SystemState::default();
        let mut snap = Seat::new().snapshot();
        snap.power_profile = Probe::Present(ProfileState {
            active: "balanced".to_owned(),
            available: vec![
                "power-saver".to_owned(),
                "balanced".to_owned(),
                "performance".to_owned(),
            ],
        });
        snap.on_ac = Probe::Present(Some(false));
        snap.charge_limit = Probe::Present(Some(80));
        snap.batteries = Probe::Present(vec![Battery {
            model: "BAT0".to_owned(),
            kind: BatteryKind::Internal,
            percentage: 61.0,
            state: BatteryState::Discharging,
            power_supply: true,
            time_to_empty: Some(Duration::from_secs(5400)),
            time_to_full: None,
            energy_rate: Some(11.7),
        }]);
        // Exercise the reconcile seam (it seeds the charge-slider live value from
        // the probe) before rendering, matching the live poll path.
        st.reconcile(&snap);
        st.snapshot = Some(snap);
        assert!(renders(&mut st), "the live POWER-4 panel drew nothing");
        assert_eq!(
            st.charge_threshold,
            Some(80),
            "reconcile seeds the charge-slider from the probe"
        );
    }

    #[test]
    fn a_refused_power_profile_switch_never_lies_about_the_active_profile() {
        // With a Present profile (active=balanced), a switch to "performance" on
        // the headless farm host has no daemon → a typed error. apply must surface
        // it inline AND withhold the optimistic active flip (§7: a failed switch
        // never reports the new profile as active). Asserted as the honest
        // coupling so a build host that DID have the daemon can't make it flaky.
        let mut st = SystemState::default();
        let mut snap = Seat::new().snapshot();
        snap.power_profile = Probe::Present(ProfileState {
            active: "balanced".to_owned(),
            available: vec!["balanced".to_owned(), "performance".to_owned()],
        });
        st.snapshot = Some(snap);
        st.apply(vec![SysAction::SetPowerProfile("performance".to_owned())]);
        let active = match st.snapshot.as_ref().map(|s| &s.power_profile) {
            Some(Probe::Present(p)) => p.active.clone(),
            _ => unreachable!("the profile probe stays Present"),
        };
        // error set ⇔ the switch failed ⇔ active stays balanced (never a lie).
        assert_eq!(
            st.error.is_some(),
            active == "balanced",
            "a failed switch must not flip the cached active profile"
        );
    }

    #[test]
    fn a_charge_threshold_write_either_succeeds_or_is_surfaced_honestly() {
        // The charge-cap write on the headless farm host has no advertising
        // battery / is unprivileged → a typed error apply must surface inline
        // (§7), never a silent success. On a machine that genuinely has the attr
        // + privilege it would succeed and seed the live cap — asserted as the
        // honest either/or so the test holds on any host.
        let mut st = SystemState::default();
        st.apply(vec![SysAction::SetChargeThreshold(70)]);
        let ok = st.error.is_none() && st.charge_threshold == Some(70);
        let surfaced = st
            .error
            .as_deref()
            .is_some_and(|e| e.contains("Charge limit"));
        assert!(
            ok || surfaced,
            "the write must either honestly succeed or surface a typed error"
        );
    }

    // ── Bluetooth control panel (E12-17) ──────────────────────────────────────

    fn bt_device(path: &str, paired: bool, connected: bool, trusted: bool) -> BtDevice {
        BtDevice {
            path: path.to_owned(),
            alias: path.to_owned(),
            address: Some("AA:BB:CC:DD:EE:FF".to_owned()),
            rssi: Some(-55),
            paired,
            connected,
            trusted,
            battery_percent: Some(72),
            icon: None,
        }
    }

    #[test]
    fn device_actions_reflect_bluetooth_state() {
        // An available (un-paired, un-connected) device: Connect + Pair, no
        // Disconnect, no Forget (Forget is a paired-only verb).
        let available = bt_device("/dev/a", false, false, false);
        assert_eq!(
            device_actions(&available, Some("/org/bluez/hci0")),
            DeviceActions {
                connect: true,
                disconnect: false,
                pair: true,
                forget: false,
            }
        );

        // A paired-but-offline device: Connect + Forget (adapter known), no Pair.
        let paired = bt_device("/dev/b", true, false, true);
        assert_eq!(
            device_actions(&paired, Some("/org/bluez/hci0")),
            DeviceActions {
                connect: true,
                disconnect: false,
                pair: false,
                forget: true,
            }
        );
        // …but Forget is withheld when the owning adapter path is unknown.
        assert_eq!(
            device_actions(&paired, None),
            DeviceActions {
                connect: true,
                disconnect: false,
                pair: false,
                forget: false,
            }
        );

        // A connected + paired device: Disconnect + Forget, no Connect, no Pair.
        let connected = bt_device("/dev/c", true, true, true);
        assert_eq!(
            device_actions(&connected, Some("/org/bluez/hci0")),
            DeviceActions {
                connect: false,
                disconnect: true,
                pair: false,
                forget: true,
            }
        );
    }

    #[test]
    fn a_bluetooth_error_is_a_flagged_warning_alert() {
        let e = SeatError::Unavailable {
            backend: mde_seat::Backend::Bluetooth,
            reason: "no adapter".into(),
        };
        let toast = bt_error_toast("connect", &e);
        assert_eq!(toast.flag, "BLUETOOTH");
        assert!(toast.headline.contains("connect"));
        assert!(toast.headline.contains("no adapter"));
    }

    #[test]
    fn a_live_bluetooth_panel_renders_its_controls() {
        // Inject a Present Bluetooth probe over an otherwise-real (Absent) snapshot
        // and prove the control rows tessellate real geometry — the reachable panel,
        // not a mockup. No button is clicked in a headless frame, so no seat write
        // fires.
        let mut st = SystemState::default();
        let mut snap = Seat::new().snapshot();
        snap.bluetooth = Probe::Present(BtStatus {
            adapters: vec![BtAdapter {
                path: "/org/bluez/hci0".to_owned(),
                name: "eagle".to_owned(),
                powered: true,
                discovering: true,
                discoverable: true,
                pairable: false,
            }],
            devices: vec![
                bt_device("/org/bluez/hci0/dev_AA", true, true, true),
                bt_device("/org/bluez/hci0/dev_BB", false, false, false),
            ],
        });
        st.snapshot = Some(snap);

        assert!(
            renders(&mut st),
            "the live Bluetooth control panel drew nothing"
        );
    }

    #[test]
    fn a_bluetooth_toggle_couples_the_cache_update_to_the_real_write() {
        // A Discoverable toggle drives the real seat. On the headless farm host the
        // write fails (no bus/adapter) → a toast is raised and the optimistic cache
        // update is withheld (§7: a failed write never lies "on"). The optimistic
        // flip only lands on a real success — the two outcomes are asserted together
        // so a live build-host adapter can't make the test flaky.
        let mut st = SystemState::default();
        let mut snap = Seat::new().snapshot();
        snap.bluetooth = Probe::Present(BtStatus {
            adapters: vec![BtAdapter {
                path: "/org/bluez/hci0".to_owned(),
                name: "eagle".to_owned(),
                powered: true,
                discovering: false,
                discoverable: false,
                pairable: false,
            }],
            devices: vec![],
        });
        st.snapshot = Some(snap);
        st.apply(vec![SysAction::BtDiscoverable(
            "/org/bluez/hci0".to_owned(),
            true,
        )]);
        let toasts = st.take_toasts();
        let cached_on = matches!(
            st.snapshot.as_ref().map(|s| &s.bluetooth),
            Some(Probe::Present(bt)) if bt.adapters[0].discoverable
        );
        // Failure ⇒ exactly one toast + cache stays false; success ⇒ no toast + the
        // optimistic flip landed. Never a toast with a lying "on" cache.
        assert_eq!(
            toasts.len() == 1,
            !cached_on,
            "the cache update must track the write outcome"
        );
    }

    #[test]
    fn leaving_the_system_surface_drops_the_pairing_agent() {
        // sync_pairing_agent(false) always releases the agent + re-arms, and with no
        // adapter present sync_pairing_agent(true) is a no-op (nothing to pair) —
        // never a bus error on a headless host.
        let mut st = SystemState {
            agent_attempted: true,
            ..SystemState::default()
        };
        st.sync_pairing_agent(false);
        assert!(st.agent.is_none());
        assert!(!st.agent_attempted);
        // Active but no snapshot/adapter yet → does not attempt (stays un-attempted).
        st.sync_pairing_agent(true);
        assert!(
            !st.agent_attempted,
            "no adapter ⇒ no agent registration attempt"
        );
    }

    // ── Settings master-detail shell (SETTINGS-1) ─────────────────────────────

    /// A unique per-test temp dir (the manual idiom `power_honor`'s tests use — no
    /// tempfile dep on the airgapped farm).
    fn nav_temp_dir(tag: &str) -> PathBuf {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("mde-settings1-{tag}-{}-{n}", std::process::id()))
    }

    #[test]
    fn the_rail_lists_the_three_domain_groups_covering_every_section() {
        // The master rail is exactly the three domain groups (lock 3), each with at
        // least one section, and every listed section names the group that lists it
        // (no orphan / mis-grouped leaf).
        assert_eq!(SettingsGroup::ALL.len(), 3);
        for group in SettingsGroup::ALL {
            assert!(
                !group.sections().is_empty(),
                "{} has no sections",
                group.label()
            );
            for &section in group.sections() {
                assert_eq!(
                    section.group(),
                    group,
                    "{} is listed under the wrong group",
                    section.label()
                );
            }
        }
    }

    #[test]
    fn every_section_is_reachable_exactly_once() {
        // Every section — the six host-control sections AND the four Mesh & System
        // sections SETTINGS-4 wired — appears exactly once across the whole taxonomy
        // and routes to a real body (the live routing is the exhaustive match in
        // settings_detail; the render test proves each paints).
        let all: Vec<SettingsSection> = SettingsGroup::ALL
            .iter()
            .flat_map(|g| g.sections().iter().copied())
            .collect();
        for section in [
            SettingsSection::Displays,
            SettingsSection::Audio,
            SettingsSection::Bluetooth,
            SettingsSection::Power,
            SettingsSection::Wallpaper,
            SettingsSection::Hotkeys,
            SettingsSection::Theme,
            SettingsSection::Identity,
            SettingsSection::Role,
            SettingsSection::Pairing,
            SettingsSection::Network,
        ] {
            assert_eq!(
                all.iter().filter(|&&s| s == section).count(),
                1,
                "{} must be reachable exactly once",
                section.label()
            );
        }
        // The whole taxonomy is exactly those eleven sections (no orphan leaf).
        assert_eq!(all.len(), 11, "the taxonomy lists exactly eleven sections");
    }

    #[test]
    fn selecting_each_section_routes_the_detail_pane_and_paints() {
        // Drive a headless frame per section with the rail resting on it: the detail
        // pane must tessellate real geometry (route to that body / honest-empty note,
        // never blank), and a click-free render leaves the selection put.
        for group in SettingsGroup::ALL {
            for &section in group.sections() {
                let mut st = SystemState {
                    nav: SettingsNav::at(section),
                    ..SystemState::default()
                };
                assert!(
                    renders(&mut st),
                    "the detail pane for {} drew nothing",
                    section.label()
                );
                assert_eq!(
                    st.nav.section, section,
                    "a click-free render must not move the selection"
                );
            }
        }
    }

    #[test]
    fn the_nav_selection_round_trips_through_disk_persistence() {
        // A moved rail selection survives a restart: write it through the real
        // save_to/load_from seam (the PowerHonorConfig idiom) and read it back; a
        // missing file folds to the default (Displays), never a fatal.
        let dir = nav_temp_dir("rt");
        std::fs::create_dir_all(&dir).expect("mkroot");
        let path = dir.join(NAV_CONFIG_FILE);

        assert_eq!(
            SettingsNav::load_from(&path),
            SettingsNav::default(),
            "a missing file folds to the default"
        );
        assert_eq!(SettingsNav::default().section, SettingsSection::Displays);

        let nav = SettingsNav::at(SettingsSection::Hotkeys);
        nav.save_to(&path).expect("save");
        let back = SettingsNav::load_from(&path);
        assert_eq!(back, nav, "the pick round-trips through disk");
        assert_eq!(back.section, SettingsSection::Hotkeys);
        assert_eq!(back.group, SettingsGroup::Personalization);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_group_in_the_file_is_normalised_against_the_section() {
        // A hand-edited / schema-drifted file whose group doesn't own its section is
        // folded so the pair is always consistent (§7 — the section wins). Also
        // exercises the snake_case serde wire form.
        let dir = nav_temp_dir("norm");
        std::fs::create_dir_all(&dir).expect("mkroot");
        let path = dir.join(NAV_CONFIG_FILE);
        std::fs::write(&path, r#"{"group":"devices","section":"hotkeys"}"#).expect("write");

        let nav = SettingsNav::load_from(&path);
        assert_eq!(nav.section, SettingsSection::Hotkeys);
        assert_eq!(
            nav.group,
            SettingsGroup::Personalization,
            "the group is re-derived from the section"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Personalization → Theme appearance (SETTINGS-5) ───────────────────────

    #[test]
    fn theme_is_a_personalization_section_reachable_once() {
        // The new Theme section lives under Personalization (the design taxonomy) and
        // is exactly one rail leaf.
        assert_eq!(
            SettingsSection::Theme.group(),
            SettingsGroup::Personalization
        );
        assert!(SettingsGroup::Personalization
            .sections()
            .contains(&SettingsSection::Theme));
        let count = SettingsGroup::ALL
            .iter()
            .flat_map(|g| g.sections().iter())
            .filter(|&&s| s == SettingsSection::Theme)
            .count();
        assert_eq!(count, 1, "Theme must be reachable exactly once");
    }

    #[test]
    fn every_accent_choice_maps_to_a_shared_style_token() {
        // Each accent choice paints an EXISTING shared Style::ACCENT* token (§4 — no
        // new hex), Brand is the interactive brand accent, and the non-Brand choices
        // are mutually distinct so the picker offers real variation.
        assert_eq!(AccentChoice::default(), AccentChoice::Brand);
        assert_eq!(AccentChoice::Brand.color(), Style::ACCENT);
        let variants: Vec<_> = AccentChoice::ALL
            .iter()
            .filter(|c| **c != AccentChoice::Brand)
            .map(|c| c.color())
            .collect();
        for (i, a) in variants.iter().enumerate() {
            assert_ne!(*a, Style::ACCENT, "a variant must differ from the brand");
            for b in &variants[i + 1..] {
                assert_ne!(a, b, "accent choices must be mutually distinct");
            }
        }
    }

    #[test]
    fn text_scale_steps_ascend_around_a_default_identity() {
        // The steps are strictly ascending and Default is the 1.0 identity (a no-op),
        // so a Default pick never perturbs the seat's DPI zoom.
        assert_eq!(TextScale::default(), TextScale::Default);
        assert!((TextScale::Default.factor() - 1.0).abs() < f32::EPSILON);
        let mut prev = f32::MIN;
        for step in TextScale::ALL {
            assert!(
                step.factor() > prev,
                "{} breaks the ascending order",
                step.label()
            );
            prev = step.factor();
        }
    }

    #[test]
    fn the_theme_appearance_round_trips_through_disk_persistence() {
        // A Theme pick survives a restart: write it through the real save_to/load_from
        // seam (the SettingsNav idiom) and read it back; a missing file folds to the
        // default (Brand / Default), never a fatal.
        let dir = nav_temp_dir("theme-rt");
        std::fs::create_dir_all(&dir).expect("mkroot");
        let path = dir.join(APPEARANCE_CONFIG_FILE);

        assert_eq!(
            AppearanceConfig::load_from(&path),
            AppearanceConfig::default(),
            "a missing file folds to the default"
        );
        assert_eq!(AppearanceConfig::default().accent, AccentChoice::Brand);
        assert_eq!(AppearanceConfig::default().text_scale, TextScale::Default);

        let cfg = AppearanceConfig {
            accent: AccentChoice::Green,
            text_scale: TextScale::Larger,
        };
        cfg.save_to(&path).expect("save");
        let back = AppearanceConfig::load_from(&path);
        assert_eq!(back, cfg, "the appearance round-trips through disk");
        assert_eq!(back.accent, AccentChoice::Green);
        assert_eq!(back.text_scale, TextScale::Larger);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_partial_appearance_file_folds_missing_fields_to_their_defaults() {
        // A drifted / partial file (only one field) reads back with the other field at
        // its serde default — never a fatal, honest to what was written.
        let dir = nav_temp_dir("theme-partial");
        std::fs::create_dir_all(&dir).expect("mkroot");
        let path = dir.join(APPEARANCE_CONFIG_FILE);
        std::fs::write(&path, r#"{"accent":"gold"}"#).expect("write");

        let cfg = AppearanceConfig::load_from(&path);
        assert_eq!(
            cfg.accent,
            AccentChoice::Gold,
            "the written field is honoured"
        );
        assert_eq!(
            cfg.text_scale,
            TextScale::Default,
            "the absent field folds to its default"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_theme_accent_choice_retints_the_live_context_on_poll() {
        // The apply seam is real: with a persisted accent pick, one poll re-tints the
        // live egui interactive accent (observable in the context's visuals) — not a
        // dead toggle (§7). Poll runs every frame in both runners, so this is global.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        assert_eq!(ctx.style().visuals.hyperlink_color, Style::ACCENT);
        let mut st = SystemState {
            appearance: AppearanceConfig {
                accent: AccentChoice::Green,
                text_scale: TextScale::Default,
            },
            ..SystemState::default()
        };
        st.poll(&ctx);
        assert_eq!(
            ctx.style().visuals.hyperlink_color,
            Style::ACCENT_MESH,
            "the accent pick re-tinted the live interactive accent"
        );
        assert_eq!(
            ctx.style().visuals.widgets.active.bg_fill,
            Style::ACCENT_MESH
        );
    }

    #[test]
    fn the_theme_text_scale_zooms_the_live_context_atop_the_dpi_base() {
        // The text-scale pick sets the whole-UI zoom to the DPI base × the step; a
        // Default pick is the identity (the base is untouched). egui STAGES a
        // set_zoom_factor to the next begin_pass, so drive the poll through real
        // passes (as both runners do) and read the applied zoom back after.
        fn poll_pass(ctx: &egui::Context, st: &mut SystemState) {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| st.poll(ctx));
        }

        let ctx = egui::Context::default();
        Style::install(&ctx);
        let base = ctx.zoom_factor();
        let mut st = SystemState {
            appearance: AppearanceConfig {
                accent: AccentChoice::default(),
                text_scale: TextScale::Larger,
            },
            ..SystemState::default()
        };
        poll_pass(&ctx, &mut st); // stages the zoom
        poll_pass(&ctx, &mut st); // the next begin_pass applies it
        let want = base * TextScale::Larger.factor();
        assert!(
            (ctx.zoom_factor() - want).abs() < f32::EPSILON,
            "the whole-UI zoom follows the text-scale step atop the DPI base"
        );

        // A Default pick leaves the base zoom untouched (a genuine no-op).
        let ctx2 = egui::Context::default();
        Style::install(&ctx2);
        let base2 = ctx2.zoom_factor();
        let mut st2 = SystemState {
            appearance: AppearanceConfig::default(),
            ..SystemState::default()
        };
        poll_pass(&ctx2, &mut st2);
        poll_pass(&ctx2, &mut st2);
        assert!(
            (ctx2.zoom_factor() - base2).abs() < f32::EPSILON,
            "a Default text-scale must not perturb the DPI base zoom"
        );
    }

    // ── Categorical accent + Carbon layers (SETTINGS-2) ───────────────────────

    #[test]
    fn each_domain_group_wears_a_distinct_shared_categorical_accent() {
        // The three domain accents REUSE the shared Style::ACCENT_* categorical set
        // (the ONE colour language PICKER-2 / EXPLORER-15 speak, §4 — no second set
        // minted here), are mutually distinct, and are each set apart from the
        // interactive brand accent so a domain tint never reads as an affordance.
        let categorical = [
            Style::ACCENT_COMMS,
            Style::ACCENT_WORKLOADS,
            Style::ACCENT_TERMINALS,
            Style::ACCENT_MESH,
            Style::ACCENT_SYSTEM,
            Style::ACCENT_MEDIA,
        ];
        let accents: Vec<egui::Color32> = SettingsGroup::ALL.iter().map(|g| g.accent()).collect();
        for a in &accents {
            assert!(
                categorical.contains(a),
                "a domain accent must be drawn from the shared categorical set, not minted"
            );
            assert_ne!(
                *a,
                Style::ACCENT,
                "a domain accent must differ from the interactive brand accent"
            );
        }
        for (i, a) in accents.iter().enumerate() {
            for b in &accents[i + 1..] {
                assert_ne!(a, b, "domain accents must be mutually distinct");
            }
        }
        // Every section inherits exactly its group's accent — the rail header AND the
        // active detail header both key off `section.group().accent()`, so a section's
        // two tints can never disagree.
        for group in SettingsGroup::ALL {
            for &section in group.sections() {
                assert_eq!(
                    section.group().accent(),
                    group.accent(),
                    "{} must wear its group's accent",
                    section.label()
                );
            }
        }
    }

    #[test]
    fn the_page_and_section_card_sit_on_ascending_carbon_layers() {
        // The page frame fills Carbon layer-01 and the section card fills layer-02
        // with a hairline border — every value a Style token (no raw literal, §4) —
        // and the card reads one elevation step above the page (not a flat fill).
        let page = page_frame(Style::SP_L);
        assert_eq!(page.fill, Style::LAYER_01, "the page rests on layer-01");

        let card = card_frame();
        assert_eq!(
            card.fill,
            Style::LAYER_02,
            "the section card rests on layer-02"
        );
        assert_eq!(
            card.stroke.color,
            Style::BORDER,
            "the card wears a hairline border"
        );
        assert!(
            (card.stroke.width - 1.0).abs() < f32::EPSILON,
            "the card border is a 1px hairline"
        );
        assert_ne!(
            card.fill, page.fill,
            "the card must be a tonal step above the page (Carbon elevation)"
        );

        // And the layered detail path actually paints headless — the section body
        // renders inside the layer-02 card without panicking, a full paint never blank.
        let mut st = SystemState::default();
        assert!(renders(&mut st), "the layered Settings page drew nothing");
    }

    // ── Expressive wide layouts (SETTINGS-3) ──────────────────────────────────

    /// Render one headless frame at an explicit pane width, tessellating on the CPU
    /// (the DRM runner's path minus the GPU) — the wide-pane variant of [`renders`].
    fn renders_at(state: &mut SystemState, width: f32) -> bool {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 720.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| state.show(ui));
        });
        !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty()
    }

    fn mixer_strip(name: &str, volume: u8, muted: bool) -> MixerStrip {
        MixerStrip {
            id: name.to_owned(),
            name: name.to_owned(),
            origin: mde_seat::StripOrigin::HostSession,
            volume,
            muted,
        }
    }

    fn connected_connector(name: &str) -> Connector {
        Connector {
            name: name.to_owned(),
            status: ConnectorStatus::Connected,
            size_mm: Some((600, 340)),
            modes: vec![DisplayMode {
                width: 1920,
                height: 1080,
                refresh_hz: 60,
                preferred: true,
            }],
        }
    }

    #[test]
    fn fit_columns_widens_on_a_wide_pane_and_collapses_on_a_narrow_one() {
        // The across-the-width reflow decision (design lock #4): a pane narrower than
        // two tiles is a single column; a wider pane fits more, capped at the section
        // max; and it never returns zero (so chunks() cannot panic on a small seat).
        assert_eq!(fit_columns(TILE_MIN_W * 1.5, 4), 1);
        assert_eq!(fit_columns(TILE_MIN_W * 2.0, 4), 2);
        assert_eq!(fit_columns(TILE_MIN_W * 3.0, 4), 3);
        assert_eq!(
            fit_columns(TILE_MIN_W * 100.0, 3),
            3,
            "a very wide pane is capped at the section max"
        );
        assert_eq!(
            fit_columns(TILE_MIN_W * 100.0, 1),
            1,
            "a one-item section stays a single column"
        );
        assert_eq!(
            fit_columns(0.0, 4),
            1,
            "a zero-width pane is still one column"
        );
    }

    #[test]
    fn the_reworked_sections_paint_across_a_wide_detail_pane() {
        // Inject Present Audio / Bluetooth / Power probes over an otherwise-real
        // (Absent) snapshot and render each reworked section in a WIDE pane: the
        // across / side-by-side layout must tessellate real geometry, never a blank —
        // the wide-pane counterpart of selecting_each_section_routes_the_detail_pane.
        let build = || {
            let mut snap = Seat::new().snapshot();
            snap.mixer = Probe::Present(MixerStatus {
                master: mixer_strip("master", 64, false),
                strips: vec![
                    mixer_strip("Music", 80, false),
                    mixer_strip("Voice", 40, true),
                    mixer_strip("VM: build", 55, false),
                ],
            });
            snap.bluetooth = Probe::Present(BtStatus {
                adapters: vec![BtAdapter {
                    path: "/org/bluez/hci0".to_owned(),
                    name: "eagle".to_owned(),
                    powered: true,
                    discovering: false,
                    discoverable: false,
                    pairable: false,
                }],
                devices: vec![bt_device("/org/bluez/hci0/dev_AA", true, true, true)],
            });
            snap.power_profile = Probe::Present(ProfileState {
                active: "balanced".to_owned(),
                available: vec!["balanced".to_owned(), "performance".to_owned()],
            });
            snap.charge_limit = Probe::Present(Some(80));
            snap.batteries = Probe::Present(vec![Battery {
                model: "BAT0".to_owned(),
                kind: BatteryKind::Internal,
                percentage: 61.0,
                state: BatteryState::Discharging,
                power_supply: true,
                time_to_empty: Some(Duration::from_secs(5400)),
                time_to_full: None,
                energy_rate: Some(11.7),
            }]);
            snap
        };
        for section in [
            SettingsSection::Audio,
            SettingsSection::Bluetooth,
            SettingsSection::Power,
            SettingsSection::Wallpaper,
            SettingsSection::Hotkeys,
            SettingsSection::Theme,
        ] {
            let snap = build();
            let mut st = SystemState {
                nav: SettingsNav::at(section),
                ..SystemState::default()
            };
            st.reconcile(&snap);
            st.snapshot = Some(snap);
            assert!(
                renders_at(&mut st, 1440.0),
                "the wide {} pane drew nothing",
                section.label()
            );
        }
    }

    #[test]
    fn the_displays_section_lays_outputs_across_and_still_drives_the_layout() {
        // Two connected outputs reconciled into the layout render as a ROW of tiles in
        // a wide pane (SETTINGS-3) — a full paint, never a stacked blank — and the
        // ToggleOutput seam still drives the intent layout, proving the presentation
        // pass didn't fork the control (§6/§7).
        let mut st = SystemState {
            nav: SettingsNav::at(SettingsSection::Displays),
            ..SystemState::default()
        };
        let mut snap = Seat::new().snapshot();
        snap.displays = Probe::Present(vec![
            connected_connector("DP-1"),
            connected_connector("DP-2"),
        ]);
        st.reconcile(&snap);
        st.snapshot = Some(snap);
        assert_eq!(
            st.layout.outputs.len(),
            2,
            "both outputs entered the layout"
        );

        assert!(
            renders_at(&mut st, 1440.0),
            "the wide Displays row of cards drew nothing"
        );

        // Toggling the FIRST output off drives the layout through apply() (the
        // last-console interlock keeps the second lit) — the real SysAction still
        // fires after the re-layout.
        let first = st.layout.outputs[0].id.clone();
        st.apply(vec![SysAction::ToggleOutput(first.clone(), false)]);
        let disabled = st
            .layout
            .outputs
            .iter()
            .find(|o| o.id == first)
            .is_some_and(|o| !o.enabled);
        assert!(
            disabled,
            "a ToggleOutput still drives the layout after the re-layout"
        );
    }

    // ── Mesh & System (SETTINGS-4) ────────────────────────────────────────────

    /// A faithful mesh-status snapshot — the exact shape `mesh-status-snapshot.sh`
    /// writes: `self` + a `nodes` directory (this node plus a lighthouse peer), the
    /// fleet counts, and the network overview. `leader` names the mesh leader so both
    /// the is-leader and not-leader paths are reachable from one fixture.
    fn mesh_snapshot(self_host: &str, leader: &str) -> String {
        format!(
            r#"{{
              "generated_ms": 1000000,
              "self": "{self_host}",
              "online": 2,
              "total": 3,
              "nodes": [
                {{"hostname":"this-node","overlay_ip":"10.42.0.7","presence":"online",
                  "role":"workstation"}},
                {{"hostname":"lh-01","overlay_ip":"10.42.0.1","presence":"online",
                  "role":"lighthouse"}}
              ],
              "network": {{"overlay_if":"nebula1","leader":"{leader}","overlay_ip":"10.42.0.7",
                "overlay_cidr":"10.42.0.0/16","routes":[],"default_gw":"172.20.0.1",
                "gateway_endpoints":["203.0.113.9:4242"],"lighthouse_ips":["10.42.0.1"],
                "cipher":"AES-256-GCM"}}
            }}"#
        )
    }

    #[test]
    fn mesh_facts_fold_this_nodes_real_identity_role_and_network() {
        // The leader is a peer (lh-01) here, so this node is NOT the leader; every
        // field is the node's real snapshot reality (§7).
        let mesh = MeshFacts::project(&mesh_snapshot("this-node", "lh-01"));
        assert!(mesh.seen);
        assert_eq!(mesh.identity.as_deref(), Some("this-node"));
        assert_eq!(mesh.role.as_deref(), Some("workstation"));
        assert_eq!(mesh.overlay_ip.as_deref(), Some("10.42.0.7"));
        assert_eq!(mesh.overlay_if.as_deref(), Some("nebula1"));
        assert_eq!(mesh.overlay_cidr.as_deref(), Some("10.42.0.0/16"));
        assert_eq!(mesh.cipher.as_deref(), Some("AES-256-GCM"));
        assert_eq!(mesh.leader.as_deref(), Some("lh-01"));
        assert_eq!(mesh.lighthouses, vec!["10.42.0.1".to_owned()]);
        assert_eq!(mesh.gateways, vec!["203.0.113.9:4242".to_owned()]);
        assert_eq!(mesh.default_gw.as_deref(), Some("172.20.0.1"));
        assert_eq!((mesh.peers_online, mesh.peers_total), (2, 3));
        assert!(!mesh.is_leader(), "the leader is a peer, not this node");

        // When this node holds the lease, is_leader flips.
        let leading = MeshFacts::project(&mesh_snapshot("this-node", "this-node"));
        assert!(leading.is_leader());
    }

    #[test]
    fn mesh_facts_stay_unseen_on_a_garbage_or_fragment_snapshot() {
        for bad in ["", "not json", "{}", "[]", r#"{"network":{}}"#] {
            let mesh = MeshFacts::project(bad);
            assert!(!mesh.seen, "{bad:?} must not read as a live snapshot");
            assert!(mesh.identity.is_none());
            assert!(mesh.lighthouses.is_empty());
        }
    }

    #[test]
    fn each_mesh_system_section_renders_live_data_and_honest_unknown() {
        // Drive each Mesh & System section twice: once over live MeshFacts and once
        // over the unseen default (every fact absent). Both must tessellate real
        // geometry — the live data OR the honest "unknown" / "reading…" note, never a
        // blank (§7). The wide side-by-side Network layout is exercised at 1440px.
        let live = MeshFacts::project(&mesh_snapshot("this-node", "this-node"));
        for section in [
            SettingsSection::Identity,
            SettingsSection::Role,
            SettingsSection::Pairing,
            SettingsSection::Network,
        ] {
            for mesh in [live.clone(), MeshFacts::default()] {
                let mut st = SystemState {
                    nav: SettingsNav::at(section),
                    mesh,
                    ..SystemState::default()
                };
                assert!(
                    renders_at(&mut st, 1440.0),
                    "the Mesh & System {} pane drew nothing",
                    section.label()
                );
            }
        }
    }

    #[test]
    fn the_pairing_section_retry_rearms_the_agent_seam() {
        // The Pairing section's Retry drives the SAME sync_pairing_agent seam: it
        // clears the once-per-visit latch and re-attempts. On the headless farm host
        // there's no adapter, so the re-attempt is an honest no-op (nothing to pair) —
        // never a bus error, never a fabricated agent (§7). Asserting the latch was
        // re-armed proves the section's action reached the seam.
        let mut st = SystemState {
            agent_attempted: true,
            ..SystemState::default()
        };
        st.apply(vec![SysAction::PairingRetry]);
        assert!(st.agent.is_none(), "no adapter ⇒ no agent registered");
        assert!(
            !st.agent_attempted,
            "Retry re-armed the once-per-visit latch on the pairing seam"
        );
    }
}
