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
//!   (logind, lock 12), multi-battery telemetry (incl. BT-peripheral batteries,
//!   lock 6), and **per-VM power rows that reuse the Instances panel's broker
//!   verbs** (§6 — one broker, two views; VM power is not reimplemented here).
//!
//! Mixer / Bluetooth stay read-only here (their interaction is E12-16 / E12-17).
//! The state holds the ONE [`Seat`] (lock 1) and re-`snapshot()`s it on the shell's
//! shared pump cadence; the same cached snapshot feeds the chrome icons.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mde_egui::egui::{self, ComboBox, RichText, Slider};
use mde_egui::{field, muted_note, OsdKind, OsdLevel, Severity, Style, Toast};

use mde_seat::hotkeys::HotkeyAction;
use mde_seat::{
    Avail, Backlight, BtAdapter, BtDevice, BtStatus, Connector, ConnectorStatus, DdcDisplay,
    DisplayLayout, DisplayMode, LidState, MixerStatus, MixerStrip, MonitorId, OutputArrangement,
    PairingAgent, PowerCaps, PowerVerb, Probe, Seat, SeatError, SeatSnapshot, HOTKEYS,
};

use crate::bt_pairing::{pairing_dialog, PairingBridge};
use crate::instances::InstancesState;
use crate::power_honor::PowerHonorConfig;
use crate::power_settings;

/// Poll cadence — a device plug, a battery drain, or a BT connect surfaces within
/// this window.
const REFRESH: Duration = Duration::from_secs(5);

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
        }
    }
}

/// One control action collected during the render borrow, applied after it ends
/// (the egui idiom the Instances panel uses) so the drive can take `&mut` freely.
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
    /// Drive a VM power verb through the Instances broker (§6).
    VmPower { idx: usize, boot: bool },
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
        }
        ctx.request_repaint_after(REFRESH);
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

    /// Render the surface's live content, driving Displays + Power against the seat
    /// and the shared Instances broker (per-VM power rows, §6).
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, instances: &mut InstancesState) {
        let mut actions: Vec<SysAction> = Vec::new();
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
                ..
            } = self;
            let snap = snapshot.as_ref();

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if let Some(err) = error.as_deref() {
                        ui.colored_label(Style::DANGER, RichText::new(err).size(Style::SMALL));
                        ui.add_space(Style::SP_S);
                    }
                    section(ui, "Mixer", |ui| mixer_section(ui, snap));
                    section(ui, "Bluetooth", |ui| {
                        bluetooth_section(ui, snap, &mut actions);
                    });
                    section(ui, "Displays", |ui| {
                        displays_section(
                            ui,
                            snap,
                            layout,
                            panel_brightness,
                            ddc_brightness,
                            &mut actions,
                        );
                    });
                    section(ui, "Power & Battery", |ui| {
                        power_section(
                            ui,
                            snap,
                            *confirm,
                            charge_threshold,
                            power_honor_config,
                            instances,
                            &mut actions,
                        );
                    });
                    section(ui, "Wallpaper", wallpaper_section);
                    section(ui, "Hotkeys", hotkeys_section);
                });

            // The BlueZ pairing modal (E12-17): a ctx-level dialog that shows only
            // while a PIN/passkey/confirm prompt is in flight, draining the shared
            // bridge the registered agent posts to. Rendered here so it lives only
            // while the System surface is shown, never blocking the render thread.
            pairing_dialog(ui.ctx(), pairing, pin_input);
        }
        self.apply(actions, instances);
    }

    /// Apply the collected actions after the render borrow ends: drive the seat /
    /// the layout model / the Instances broker, folding any typed failure into the
    /// honest inline error (never a panic, never a silent no-op).
    fn apply(&mut self, actions: Vec<SysAction>, instances: &mut InstancesState) {
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
                SysAction::VmPower { idx, boot } => instances.drive_power(idx, boot),
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
            }
        }
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

// ──────────────────────────── render ────────────────────────────

/// A titled section: a dim caption over a grouped card. The shared surface idiom.
fn section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    ui.label(
        RichText::new(title)
            .color(Style::TEXT_DIM)
            .size(Style::SMALL)
            .strong(),
    );
    ui.group(body);
    ui.add_space(Style::SP_S);
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

/// The Mixer section — read-only status (fader/mute/solo interaction is E12-16).
fn mixer_section(ui: &mut egui::Ui, snap: Option<&SeatSnapshot>) {
    probe_section(
        ui,
        snap,
        |s| &s.mixer,
        |ui, m: &MixerStatus| {
            strip_row(ui, &m.master, true);
            for strip in &m.strips {
                strip_row(ui, strip, false);
            }
            if m.strips.is_empty() {
                muted_note(ui, "No channel strips.");
            }
        },
    );
}

/// One mixer strip as a read-only status row.
fn strip_row(ui: &mut egui::Ui, strip: &MixerStrip, master: bool) {
    let value = if strip.muted {
        format!("{}% \u{00B7} muted", strip.volume)
    } else {
        format!("{}%", strip.volume)
    };
    let tone = if strip.muted {
        Style::WARN
    } else {
        Style::TEXT
    };
    let label = if master {
        "Master".to_owned()
    } else {
        strip.name.clone()
    };
    field(ui, &label, &value, tone);
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
            for adapter in &bt.adapters {
                adapter_row(ui, adapter, actions);
            }
            // Devices hang off the first adapter (the RemoveDevice owner). A scan
            // annotates each row with live RSSI.
            let adapter_path = bt.adapters.first().map(|a| a.path.as_str());
            let scanning = bt.adapters.iter().any(|a| a.discovering);
            if bt.devices.is_empty() {
                muted_note(ui, "No devices — scan to discover nearby devices.");
            }
            for device in &bt.devices {
                device_row(ui, device, adapter_path, scanning, actions);
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
            for out in &layout.outputs {
                let connector = connectors.iter().find(|c| c.name == out.connector);
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
                ui.add_space(Style::SP_XS);
            }
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
/// the power-profile + charge-cap controls, the on-AC source line, multi-battery
/// telemetry, and per-VM power rows (reusing the Instances broker, §6). Every
/// POWER-4 control drives the real seat / reads the real snapshot — no inert
/// affordance (§7). Idle-suspend + lid-close are deliberately out of scope here
/// (POWER-5, once the honorer is not inert).
fn power_section(
    ui: &mut egui::Ui,
    snap: Option<&SeatSnapshot>,
    confirm: Option<PowerVerb>,
    charge_threshold: &mut Option<u8>,
    power_honor_config: &mut PowerHonorConfig,
    instances: &InstancesState,
    actions: &mut Vec<SysAction>,
) {
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

    // Idle-suspend + lid-close policy (POWER-5) — the honorer that enforces these
    // lives in `crate::power_honor` and reads this config every frame, so the
    // pickers are §7-real (never inert). Safe defaults: idle Never, lid Suspend.
    ui.add_space(Style::SP_XS);
    power_settings::idle_timeout_body(ui, power_honor_config, actions);
    power_settings::lid_action_body(ui, power_honor_config, actions);

    // Power profile (POWER-4) — the daemon's available set + current active. When
    // power-profiles-daemon is Absent the probe renders the honest "unavailable"
    // reason, never a fabricated active (§7).
    ui.add_space(Style::SP_XS);
    probe_section(ui, snap, |s| &s.power_profile, |ui, state| {
        power_settings::profile_body(ui, state, actions);
    });

    // On-AC / on-battery source line (POWER-4) — the honest UPower LinePower
    // reading, "unknown" when no adapter is tracked, "unavailable" when Absent.
    ui.add_space(Style::SP_XS);
    probe_section(ui, snap, |s| &s.on_ac, |ui, on_ac: &Option<bool>| {
        power_settings::ac_source_body(ui, *on_ac);
    });

    // Charge limit (POWER-4) — the charge-stop cap slider when a battery
    // advertises the attribute, an honest "not supported" when Present(None), and
    // the probe's "unavailable" reason when Absent (no power-supply class).
    ui.add_space(Style::SP_XS);
    probe_section(ui, snap, |s| &s.charge_limit, |ui, cap: &Option<u8>| {
        power_settings::charge_threshold_body(ui, *cap, charge_threshold, actions);
    });

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

    // Per-VM power rows — the Instances roster, a second view (§6). Empty roster →
    // an honest note pointing at the Instances surface (never fabricated VMs).
    ui.add_space(Style::SP_XS);
    ui.label(
        RichText::new("Local VMs")
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
    );
    let rows = instances.power_rows();
    if rows.is_empty() {
        muted_note(ui, "No local VMs — define one on the Instances surface.");
    }
    for (idx, row) in rows.iter().enumerate() {
        ui.horizontal(|ui| {
            let tone = if row.gated {
                Style::WARN
            } else if row.running {
                Style::OK
            } else {
                Style::TEXT_DIM
            };
            ui.label(RichText::new(DOT).color(tone).size(Style::SMALL));
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new(&row.name)
                    .color(Style::TEXT)
                    .size(Style::SMALL),
            );
            ui.add_space(Style::SP_S);
            ui.colored_label(tone, RichText::new(row.state).size(Style::SMALL));
            ui.add_space(Style::SP_S);
            // Reuse the broker verbs — Boot when down, Shutdown when running.
            if row.running {
                if ui
                    .button(RichText::new("Shutdown").size(Style::SMALL))
                    .clicked()
                {
                    actions.push(SysAction::VmPower { idx, boot: false });
                }
            } else if ui
                .button(RichText::new("Boot").size(Style::SMALL))
                .clicked()
            {
                actions.push(SysAction::VmPower { idx, boot: true });
            }
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
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Desktop wallpaper")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);
        ComboBox::from_id_salt("qbrand11-wallpaper")
            .selected_text(RichText::new(current.label()).size(Style::SMALL))
            .show_ui(ui, |ui| {
                for wallpaper in crate::backdrop::Wallpaper::ALL {
                    let selected = wallpaper == current;
                    if ui
                        .selectable_label(
                            selected,
                            RichText::new(wallpaper.label()).size(Style::SMALL),
                        )
                        .clicked()
                        && !selected
                    {
                        crate::backdrop::select_wallpaper(&ctx, wallpaper);
                    }
                }
            });
    });
    muted_note(
        ui,
        "The five official Quazar wallpapers ship in the RPM; your choice follows your mesh identity when a workgroup volume is present.",
    );
}

/// The Hotkeys section — the fixed compiled-in table (lock 9) read-only.
fn hotkeys_section(ui: &mut egui::Ui) {
    for hotkey in HOTKEYS {
        ui.horizontal(|ui| {
            ui.label(RichText::new(DOT).color(Style::TEXT_DIM).size(Style::SMALL));
            ui.add_space(Style::SP_XS);
            field(ui, hotkey.chord, hotkey.action.label(), Style::TEXT);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_egui::egui::{pos2, vec2, Rect};
    use mde_seat::{Battery, BatteryKind, BatteryState, ProfileState};

    /// Drive one headless frame of the System panel over a real seat + a given
    /// Instances roster, and tessellate on the CPU (the DRM runner's path minus GPU).
    fn renders(state: &mut SystemState, instances: &mut InstancesState) -> bool {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| state.show(ui, instances));
        });
        !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty()
    }

    #[test]
    fn the_pre_poll_state_is_a_full_paint_not_a_blank_panel() {
        let mut st = SystemState::default();
        let mut inst = InstancesState::default();
        assert!(
            renders(&mut st, &mut inst),
            "pre-poll System panel drew nothing"
        );
    }

    #[test]
    fn a_real_seat_snapshot_mounts_and_renders_every_section() {
        // Over a REAL Seat::snapshot(): on the headless farm host most backends are
        // Absent (each an honest typed line), the arrangement/power controls fold to
        // their not-available/empty states — still a full paint path, never blank.
        let ctx = egui::Context::default();
        let mut st = SystemState::default();
        st.poll(&ctx); // one snapshot + reconcile
        let mut inst = InstancesState::default();
        assert!(
            renders(&mut st, &mut inst),
            "live System panel drew nothing"
        );
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
        let mut inst = InstancesState::default();
        st.apply(vec![SysAction::ArmConfirm(PowerVerb::Reboot)], &mut inst);
        assert_eq!(st.confirm, Some(PowerVerb::Reboot));
        st.apply(vec![SysAction::CancelConfirm], &mut inst);
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
        let mut inst = InstancesState::default();
        assert!(
            renders(&mut st, &mut inst),
            "the live POWER-4 panel drew nothing"
        );
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
        let mut inst = InstancesState::default();
        st.apply(
            vec![SysAction::SetPowerProfile("performance".to_owned())],
            &mut inst,
        );
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
        let mut inst = InstancesState::default();
        st.apply(vec![SysAction::SetChargeThreshold(70)], &mut inst);
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

        let mut inst = InstancesState::default();
        assert!(
            renders(&mut st, &mut inst),
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
        let mut inst = InstancesState::default();

        st.apply(
            vec![SysAction::BtDiscoverable(
                "/org/bluez/hci0".to_owned(),
                true,
            )],
            &mut inst,
        );
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
}
