//! `Surface::System` — this seat's host-controls panel (E12-15 status; E12-18 makes
//! Displays + Power interactive).
//!
//! Under E12 "Construct" the shell owns the DRM seat with no compositor and no
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
//!   lock 6). VM lifecycle now lives in the CONSTRUCT-CLOUD plane, not a local
//!   cloud-hypervisor broker.
//!
//! Mixer / Bluetooth stay read-only here (their interaction is E12-16 / E12-17).
//! The state holds the ONE [`Seat`] (lock 1) and re-`snapshot()`s it on the shell's
//! shared pump cadence; the same cached snapshot feeds the chrome icons.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mde_egui::egui::{self, ComboBox, RichText, Slider};
use mde_egui::style::Elevation;
use mde_egui::{
    field, muted_note, InputPolicy, Motion, MotionMode, OsdKind, OsdLevel, Severity, Style,
    StyleColorScheme, Toast,
};
use mde_theme::brand::icons::IconId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use mde_seat::hotkeys::HotkeyAction;
use mde_seat::{
    Avail, Backlight, BtAdapter, BtDevice, BtStatus, Connector, ConnectorStatus, DdcDisplay,
    DisplayLayout, DisplayMode, LidState, MixerStatus, MixerStrip, MonitorId, OutputArrangement,
    PairingAgent, PowerCaps, PowerVerb, Probe, Seat, SeatError, SeatSnapshot, HOTKEYS,
};

use crate::bt_pairing::{pairing_dialog, PairingBridge};
use crate::dock::icon_texture;
use crate::power_honor::PowerHonorConfig;
use crate::power_settings;
use crate::seat_pump::{connector_key, SnapshotPump};

const SYSTEM_READING_SEAT_COPY: &str = "Reading the seat...";
const SYSTEM_SCANNING_COPY: &str = "Scanning...";
const SYSTEM_MESH_READING_COPY: &str = "Reading this node's mesh status...";
const DISPLAY_NUDGE_LEFT_ICON: IconId = IconId::ArrowLeft;
const DISPLAY_NUDGE_RIGHT_ICON: IconId = IconId::ArrowRight;
const SETTINGS_TOOLTIP_W: f32 = 260.0;

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
    /// The one seat over the real host hardware (in-process, lock 1). On the render
    /// thread it now drives only the **control verbs** (backlight / DDC / power /
    /// mixer / Bluetooth writes); the read-only `snapshot()` moved off-thread to the
    /// [`pump`](Self::pump) so the slow I2C/DBus probes never freeze a frame (perf-2).
    seat: Seat,
    /// The latest snapshot, drained from the off-thread [`pump`](Self::pump). `None`
    /// until the pump publishes its first one (within a frame of spawn).
    snapshot: Option<SeatSnapshot>,
    /// The background snapshot producer (perf-2): a dedicated thread over its own
    /// read-only seat that publishes the newest [`SeatSnapshot`] over a channel the
    /// render thread drains. Spawned lazily on the first [`poll`](Self::poll) (it
    /// needs the `egui::Context` to wake the render thread), dropped — which stops +
    /// joins the thread — with the surface.
    pump: Option<SnapshotPump>,
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
    /// Mesh & System → Remote Proofing policy for Sunshine/Moonlight console
    /// shadowing. This keeps exposure, pairing, capture, encoder, indicator/input,
    /// and VNC fallback in the Settings workspace instead of hand-edited service
    /// config. The service lifecycle layer consumes this persisted policy.
    remote_proofing: RemoteProofingConfig,
    /// Devices → Mouse & Touch policy for pointer speed, handedness, scrolling,
    /// double-click timing, and touch gesture gates. Loaded from disk on start,
    /// saved on change, and pushed into the native DRM input policy every poll so
    /// sensitivity fixes affect the real seat rather than only the Settings UI.
    mouse_touch: MouseTouchConfig,
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
    /// The seat's baseline egui `animation_time`, captured ONCE on the first appearance
    /// apply so the reduce-motion preference (a11y-07) can restore the real motion
    /// cadence when it turns back OFF — rather than a guessed constant. Same
    /// capture-once idiom as [`zoom_base`](Self::zoom_base). `None` until the first poll
    /// observes the base.
    animation_base: Option<f32>,
}

impl Default for SystemState {
    fn default() -> Self {
        Self {
            seat: Seat::new(),
            snapshot: None,
            pump: None,
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
            remote_proofing: RemoteProofingConfig::load(),
            mouse_touch: MouseTouchConfig::load(),
            appearance: AppearanceConfig::load(),
            zoom_base: None,
            animation_base: None,
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
    /// The poll seam: drain the newest OFF-THREAD snapshot (never blocking on the
    /// probe — perf-2), then reconcile the arrangement model + brightness seeds
    /// against it.
    ///
    /// The expensive `seat.snapshot()` (pw-dump + the I2C DDC/CI probe + the
    /// system-bus reads) used to run inline here every 5 s, freezing the frame for the probe's
    /// duration. It now runs on the [`SnapshotPump`] background thread; the render
    /// thread only drains the latest published snapshot — a `try_recv`, so this can
    /// never block. Data freshness is unchanged from the UI's view: the pump wakes
    /// the render thread on each publish, so a fresh snapshot lands within a frame.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        // Spawn the pump once (it needs the ctx to wake us on each publish).
        if self.pump.is_none() {
            self.pump = Some(SnapshotPump::spawn(ctx.clone()));
        }
        // Drain to the newest published snapshot — non-blocking. `None` means nothing
        // new arrived, so the surface keeps the snapshot it already holds.
        if let Some(snap) = self.pump.as_ref().and_then(SnapshotPump::drain_latest) {
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
        // Devices → Mouse & Touch: publish the persisted input policy to the bare DRM
        // seat and egui input options every frame, so a restart restores pointer speed
        // and double-click timing before the operator reopens Settings.
        self.apply_mouse_touch(ctx);
        ctx.request_repaint_after(REFRESH);
    }

    /// Apply the persisted Personalization → Theme appearance (SETTINGS-5) to the live
    /// context: install the selected colour mode, re-tint the interactive accent, hold
    /// the whole-UI text-scale zoom, and damp motion under the reduce-motion preference
    /// (a11y-07). Cheap-guarded — a no-op frame costs field reads and never re-mutates
    /// — and self-correcting: if a formfactor [`Style::install_with_density`] re-install
    /// resets the look, the next poll re-applies the pick. Every effect is real runtime
    /// state (the egui visuals + zoom + animation time + the shared `Motion` global),
    /// never a dead toggle (§7).
    fn apply_appearance(&mut self, ctx: &egui::Context) {
        // Colour mode + accent — re-tint only when the live look drifts from the pick
        // (a settings change, a fresh context, OR a formfactor re-install reset it).
        let want_scheme = self.appearance.color_scheme.runtime();
        let want_accent = self.appearance.accent.color();
        let want_live_accent = Style::accent_for_scheme(want_scheme, want_accent);
        if Style::color_scheme(ctx) != want_scheme
            || ctx.style().visuals.hyperlink_color != want_live_accent
        {
            Style::set_color_scheme_and_accent(ctx, want_scheme, want_accent);
        }
        // Text-scale — capture the seat's DPI zoom base once (what the DRM runner set
        // from the panel, or 1.0 windowed), then hold the zoom at base × the chosen
        // step so the accessibility scale COMPOSES with HiDPI instead of clobbering it.
        let base = *self.zoom_base.get_or_insert_with(|| ctx.zoom_factor());
        let want_zoom = base * self.appearance.text_scale.factor();
        if (ctx.zoom_factor() - want_zoom).abs() > f32::EPSILON {
            ctx.set_zoom_factor(want_zoom);
        }
        // Motion mode (MOTION-DRM-5) — drive BOTH runtime seams from the persisted
        // choice: the shared typed `Motion` subsystem gets the full normal/reduced/
        // disabled enum, while egui's built-in animation_time remains a conservative
        // legacy damping signal for old call sites that only understand "reduced".
        // The baseline cadence is captured once, like the zoom base, so returning to
        // Normal restores the seat's real animation time rather than a guess.
        let motion_mode = self.appearance.motion_mode.runtime();
        Motion::set_mode(motion_mode);
        let anim_base = *self
            .animation_base
            .get_or_insert_with(|| ctx.style().animation_time);
        let want_anim = if motion_mode == MotionMode::Normal {
            anim_base
        } else {
            0.0
        };
        if (ctx.style().animation_time - want_anim).abs() > f32::EPSILON {
            ctx.style_mut(|s| s.animation_time = want_anim);
        }
    }

    /// Apply the persisted Devices → Mouse & Touch policy to the live input seams.
    /// Pointer/scroll/button/touch gates publish through `mde_egui::InputPolicy`,
    /// which the bare DRM libinput loop reads as it translates events. Double-click
    /// timing is an egui context option, so windowed and DRM paths share it.
    fn apply_mouse_touch(&self, ctx: &egui::Context) {
        mde_egui::set_input_policy(self.mouse_touch.input_policy());
        let want = f64::from(self.mouse_touch.double_click_ms) / 1000.0;
        ctx.options_mut(|o| {
            if (o.input_options.max_double_click_delay - want).abs() > f64::EPSILON {
                o.input_options.max_double_click_delay = want;
            }
        });
    }

    /// Rebuild the layout on a connector-set change (a replug) and seed any newly
    /// seen brightness value — without clobbering an in-flight operator edit.
    fn reconcile(&mut self, snap: &SeatSnapshot) {
        if let Probe::Present(connectors) = &snap.displays {
            // The SAME connector-set signal the snapshot pump's DDC cache keys its
            // `ddcutil detect` on (perf-2), so a re-plug rebuilds the layout here AND
            // re-detects DDC there off one shared derivation.
            let key = connector_key(connectors);
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

    /// Persisted Win10-hybrid taskbar auto-hide preference. The shell mirrors
    /// this into [`crate::dock::DockState`] each frame, keeping Settings as the
    /// source of truth while the dock owns the animation and strut behavior.
    pub(crate) const fn taskbar_autohide(&self) -> bool {
        self.appearance.taskbar_autohide
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
        let remote_proofing_before = self.remote_proofing;
        let mouse_touch_before = self.mouse_touch;
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
                remote_proofing,
                mouse_touch,
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
                                remote_proofing,
                                mouse_touch,
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
        // Persist Remote Proofing policy changes from Mesh & System. The live
        // Sunshine/service bridge consumes this config; Settings owns the operator
        // source of truth.
        if self.remote_proofing != remote_proofing_before {
            self.remote_proofing.save();
        }
        // Persist Devices → Mouse & Touch changes and publish them immediately so
        // pointer speed / button mapping / scroll direction adjust without waiting
        // for the next poll tick.
        if self.mouse_touch != mouse_touch_before {
            self.mouse_touch.save();
            self.apply_mouse_touch(ui.ctx());
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
            | HotkeyAction::OpenSystem
            | HotkeyAction::OpenOmnibox
            | HotkeyAction::MediaPlayPause
            | HotkeyAction::MediaPause
            | HotkeyAction::MediaStop
            | HotkeyAction::MediaNext
            | HotkeyAction::MediaPrevious => None,
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

/// One rail leaf of the Settings master-detail shell (SETTINGS-1): the host-device
/// control sections plus the Mesh & System sections SETTINGS-4 wired to
/// this node's real identity / role / pairing / network state. Each belongs to
/// exactly one [`SettingsGroup`]; the pair the rail rests on is a [`SettingsNav`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SettingsSection {
    /// Per-output enable / mode / arrangement + brightness (`displays_section`).
    #[default]
    Displays,
    /// Pointer, scroll, touch, and Surface-class gesture policy.
    Mouse,
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
    /// Sunshine/Moonlight console shadowing policy (`remote_proofing_section`).
    RemoteProofing,
}

impl SettingsSection {
    /// The rail + detail-header label.
    const fn label(self) -> &'static str {
        match self {
            Self::Displays => "Displays",
            Self::Mouse => "Mouse & Touch",
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
            Self::RemoteProofing => "Remote Proofing",
        }
    }

    /// The YAMIS-backed glyph used anywhere the Settings shell needs a compact
    /// visual handle for this section.
    const fn icon_id(self) -> IconId {
        match self {
            Self::Displays => IconId::DisplaySettings,
            Self::Mouse => IconId::Mouse,
            Self::Audio => IconId::Audio,
            Self::Bluetooth => IconId::Bluetooth,
            Self::Power => IconId::PowerBattery,
            Self::Wallpaper => IconId::Wallpaper,
            Self::Hotkeys => IconId::Keyboard,
            Self::Theme => IconId::Appearance,
            Self::Identity => IconId::Node,
            Self::Role => IconId::Workstation,
            Self::Pairing => IconId::Share,
            Self::Network => IconId::NetworkSettings,
            Self::RemoteProofing => IconId::PictureInPicture,
        }
    }

    /// The domain group this section lives under (the single source of truth the
    /// rail + [`SettingsNav`] normalise against).
    const fn group(self) -> SettingsGroup {
        match self {
            Self::Displays | Self::Mouse | Self::Audio | Self::Bluetooth | Self::Power => {
                SettingsGroup::Devices
            }
            Self::Wallpaper | Self::Hotkeys | Self::Theme => SettingsGroup::Personalization,
            Self::Identity | Self::Role | Self::Pairing | Self::Network | Self::RemoteProofing => {
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
    /// Displays · Mouse & Touch · Audio · Bluetooth · Power & Battery.
    #[default]
    Devices,
    /// Wallpaper · Hotkeys · Theme.
    Personalization,
    /// Identity · Role · Pairing · Network · Remote Proofing (SETTINGS-4).
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
                SettingsSection::Mouse,
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
                SettingsSection::RemoteProofing,
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

// ──────────────────────────── Devices → Mouse & Touch ────────────────────────────

/// The client-data-dir file Devices → Mouse & Touch persists to. This shell-side
/// policy feeds the native DRM input translator and mirrors the existing `mackesd`
/// input key names/ranges for the compositor-backed path.
const MOUSE_TOUCH_CONFIG_FILE: &str = "settings-mouse-touch.json";

/// Which physical button acts as the primary pointer button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PrimaryButton {
    /// Standard right-handed mapping.
    #[default]
    Left,
    /// Left-handed mapping: physical right button activates primary actions.
    Right,
}

impl PrimaryButton {
    const ALL: [Self; 2] = [Self::Left, Self::Right];

    const fn label(self) -> &'static str {
        match self {
            Self::Left => "Left",
            Self::Right => "Right",
        }
    }

    const fn left_handed(self) -> bool {
        matches!(self, Self::Right)
    }
}

/// Devices → Mouse & Touch persisted policy. Defaults slow the pointer slightly
/// because the native DRM seat's raw libinput deltas are too sensitive on current
/// proof hardware; every field is clamped on load so hand-edited drift cannot poison
/// the input loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct MouseTouchConfig {
    /// Pointer speed percent delta. `-35` is the platform default until per-device
    /// calibration lands; `0` means unchanged libinput motion.
    #[serde(default = "default_pointer_speed_percent")]
    pointer_speed_percent: i16,
    /// Primary button mapping.
    #[serde(default)]
    primary_button: PrimaryButton,
    /// Reverse wheel/touchpad/touch gesture scroll deltas.
    #[serde(default)]
    natural_scroll: bool,
    /// Wheel/touchpad/touch scroll delta multiplier, percent.
    #[serde(default = "default_scroll_speed_percent")]
    scroll_speed_percent: u16,
    /// egui double-click recognition window.
    #[serde(default = "default_double_click_ms")]
    double_click_ms: u16,
    /// Touchpad tap-to-click policy for compositor-backed seats.
    #[serde(default = "default_true")]
    touchpad_tap_to_click: bool,
    /// Two-finger scroll gestures from touchpads/touchscreens.
    #[serde(default = "default_true")]
    two_finger_scroll: bool,
    /// Direct touchscreen contacts on Surface-class hardware.
    #[serde(default = "default_true")]
    touchscreen_enabled: bool,
    /// Edge swipes that reveal shell affordances.
    #[serde(default = "default_true")]
    edge_gestures: bool,
    /// Long-press gesture synthesizes a secondary click.
    #[serde(default = "default_true")]
    long_press_secondary: bool,
}

const fn default_pointer_speed_percent() -> i16 {
    -35
}

const fn default_scroll_speed_percent() -> u16 {
    100
}

const fn default_double_click_ms() -> u16 {
    300
}

impl Default for MouseTouchConfig {
    fn default() -> Self {
        Self {
            pointer_speed_percent: default_pointer_speed_percent(),
            primary_button: PrimaryButton::Left,
            natural_scroll: false,
            scroll_speed_percent: default_scroll_speed_percent(),
            double_click_ms: default_double_click_ms(),
            touchpad_tap_to_click: true,
            two_finger_scroll: true,
            touchscreen_enabled: true,
            edge_gestures: true,
            long_press_secondary: true,
        }
    }
}

impl MouseTouchConfig {
    /// The default Mouse & Touch path (`<client-data-dir>/...json`), or `None` when
    /// no data dir resolves.
    fn default_path() -> Option<PathBuf> {
        mde_bus::client_data_dir().map(|d| d.join(MOUSE_TOUCH_CONFIG_FILE))
    }

    fn normalized(mut self) -> Self {
        self.pointer_speed_percent = self.pointer_speed_percent.clamp(-100, 100);
        self.scroll_speed_percent = self.scroll_speed_percent.clamp(25, 300);
        self.double_click_ms = self.double_click_ms.clamp(150, 900);
        self
    }

    /// Runtime policy consumed by `mde_egui`'s bare DRM input loop.
    fn input_policy(self) -> InputPolicy {
        let cfg = self.normalized();
        InputPolicy {
            pointer_speed_percent: cfg.pointer_speed_percent,
            scroll_speed_percent: cfg.scroll_speed_percent,
            left_handed: cfg.primary_button.left_handed(),
            natural_scroll: cfg.natural_scroll,
            touchscreen_enabled: cfg.touchscreen_enabled,
            two_finger_scroll: cfg.two_finger_scroll,
            edge_gestures: cfg.edge_gestures,
            long_press_secondary: cfg.long_press_secondary,
        }
    }

    /// Value compatible with `mackesd`'s `mouse.pointer_accel` setting.
    fn mackesd_pointer_accel(self) -> f64 {
        f64::from(self.normalized().pointer_speed_percent) / 100.0
    }

    /// Load from `path`, folding missing / malformed data to sensitivity-safe defaults.
    fn load_from(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Self>(&s).ok())
            .map_or_else(Self::default, Self::normalized)
    }

    /// Load from the default path.
    #[must_use]
    fn load() -> Self {
        Self::default_path().map_or_else(Self::default, |p| Self::load_from(&p))
    }

    /// Write to `path` atomically, mirroring [`SettingsNav`].
    ///
    /// # Errors
    /// The [`std::io::Error`] if the dir cannot be created or the file cannot be
    /// written / renamed.
    fn save_to(self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&self.normalized())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Persist to the default path (silent no-op when no data dir resolves).
    fn save(self) {
        if let Some(path) = Self::default_path() {
            let _ = self.save_to(&path);
        }
    }
}

// ──────────────────────────── Mesh & System → Remote Proofing ────────────────────────────

/// The client-data-dir file Mesh & System → Remote Proofing persists to. This is the
/// single shell-side policy source for Sunshine/Moonlight console shadowing; the
/// service/provisioning layer consumes it rather than scattering knobs across the
/// Browser, worklist, or hand-edited Sunshine config.
const REMOTE_PROOFING_CONFIG_FILE: &str = "settings-remote-proofing.json";

/// The network surface a Sunshine/Moonlight proofing service may expose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RemoteProofingExposure {
    /// Bind only to the encrypted overlay / mesh path.
    #[default]
    MeshOnly,
    /// Bind to the trusted LAN for local Moonlight clients.
    Lan,
    /// Bind broadly; requires an explicit warning in the Settings UI.
    Public,
}

impl RemoteProofingExposure {
    const ALL: [Self; 3] = [Self::MeshOnly, Self::Lan, Self::Public];

    const fn label(self) -> &'static str {
        match self {
            Self::MeshOnly => "Mesh only",
            Self::Lan => "LAN",
            Self::Public => "All interfaces",
        }
    }

    const fn description(self) -> &'static str {
        match self {
            Self::MeshOnly => "Encrypted mesh path; default for workstation proofing.",
            Self::Lan => "Trusted local network for nearby Moonlight clients.",
            Self::Public => "Broad bind; keep behind explicit firewall policy.",
        }
    }
}

/// Preferred Sunshine capture path for the native shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RemoteProofingCapture {
    Auto,
    #[default]
    Kms,
    Wlr,
    X11,
}

impl RemoteProofingCapture {
    const ALL: [Self; 4] = [Self::Kms, Self::Auto, Self::Wlr, Self::X11];

    const fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Kms => "DRM/KMS",
            Self::Wlr => "Wayland DMA-BUF",
            Self::X11 => "X11 fallback",
        }
    }

    const fn description(self) -> &'static str {
        match self {
            Self::Auto => "Let Sunshine pick the first available capture method.",
            Self::Kms => "Native DRM shell capture; preferred proofing path.",
            Self::Wlr => "wlroots/Wayland capture path for compositor seats.",
            Self::X11 => "Compatibility fallback; avoid for performance proofing.",
        }
    }

    const fn sunshine_value(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Kms => "kms",
            Self::Wlr => "wlr",
            Self::X11 => "x11",
        }
    }
}

/// Preferred hardware encoder family for the Sunshine stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RemoteProofingEncoder {
    #[default]
    Auto,
    IntelVaapi,
    NvidiaNvenc,
    AmdVce,
    Software,
}

impl RemoteProofingEncoder {
    const ALL: [Self; 5] = [
        Self::Auto,
        Self::IntelVaapi,
        Self::NvidiaNvenc,
        Self::AmdVce,
        Self::Software,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::IntelVaapi => "Intel VAAPI",
            Self::NvidiaNvenc => "NVIDIA NVENC",
            Self::AmdVce => "AMD VCE",
            Self::Software => "Software",
        }
    }

    const fn description(self) -> &'static str {
        match self {
            Self::Auto => "Let Sunshine use the first hardware encoder available.",
            Self::IntelVaapi => "Intel hardware encode path used by the .15 proof seat.",
            Self::NvidiaNvenc => "NVIDIA hardware encoder.",
            Self::AmdVce => "AMD hardware encoder.",
            Self::Software => "CPU fallback for bring-up only.",
        }
    }

    const fn sunshine_value(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::IntelVaapi => "vaapi",
            Self::NvidiaNvenc => "nvenc",
            Self::AmdVce => "amdvce",
            Self::Software => "software",
        }
    }
}

/// The effective network bind the Sunshine provisioning bridge should apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteProofingBindScope {
    Disabled,
    MeshOnly,
    Lan,
    Public,
}

impl RemoteProofingBindScope {
    const fn label(self) -> &'static str {
        match self {
            Self::Disabled => "Disabled",
            Self::MeshOnly => "Mesh overlay",
            Self::Lan => "Trusted LAN",
            Self::Public => "All interfaces",
        }
    }
}

/// The firewall/exposure policy paired with the bind scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteProofingFirewallPolicy {
    Closed,
    MeshOverlayOnly,
    TrustedLanOnly,
    PublicExplicit,
}

impl RemoteProofingFirewallPolicy {
    const fn label(self) -> &'static str {
        match self {
            Self::Closed => "Closed",
            Self::MeshOverlayOnly => "Mesh ports only",
            Self::TrustedLanOnly => "Trusted LAN ports",
            Self::PublicExplicit => "Public ports with warning",
        }
    }
}

/// The render-free service/provisioning plan derived from the Settings policy.
///
/// This is the product-code contract missing from the hand-configured `.15` proof:
/// Settings owns the durable operator choice, while a later service bridge can consume
/// this one plan to write Sunshine config, firewall state, prompts, indicators, and
/// remote-input gates without reinterpreting UI widgets.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RemoteProofingServicePlan {
    enabled: bool,
    bind_scope: RemoteProofingBindScope,
    bind_address: Option<String>,
    firewall: RemoteProofingFirewallPolicy,
    sunshine_capture: &'static str,
    sunshine_encoder: &'static str,
    min_fps_target: u8,
    native_pairing_prompt: bool,
    require_local_approval: bool,
    show_shadowing_indicator: bool,
    allow_remote_input: bool,
    vnc_fallback: bool,
    warnings: Vec<&'static str>,
}

/// Mesh & System → Remote Proofing policy. These controls are intentionally grouped
/// together: exposure, pairing, capture, encode, indicator/input, and VNC fallback
/// form one operator decision surface for console shadowing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct RemoteProofingConfig {
    /// Whether the workstation should offer Sunshine/Moonlight shadowing at all.
    #[serde(default)]
    enabled: bool,
    /// Where the Sunshine service may bind.
    #[serde(default)]
    exposure: RemoteProofingExposure,
    /// Capture method requested from Sunshine.
    #[serde(default)]
    capture: RemoteProofingCapture,
    /// Encoder family requested from Sunshine.
    #[serde(default)]
    encoder: RemoteProofingEncoder,
    /// Native shell approval prompt for Moonlight pairing.
    #[serde(default = "default_true")]
    native_pairing_prompt: bool,
    /// Require local approval before a remote viewer can attach.
    #[serde(default = "default_true")]
    require_local_approval: bool,
    /// Show an on-seat indicator while a remote viewer is attached.
    #[serde(default = "default_true")]
    show_shadowing_indicator: bool,
    /// Allow remote keyboard/mouse input once authorized.
    #[serde(default = "default_true")]
    allow_remote_input: bool,
    /// Keep VNC as a fallback/admin channel, not the primary proofing surface.
    #[serde(default = "default_true")]
    vnc_fallback: bool,
    /// Minimum frame cadence expected for proofing, clamped on load.
    #[serde(default = "default_min_fps")]
    min_fps_target: u8,
}

const fn default_true() -> bool {
    true
}

const fn default_min_fps() -> u8 {
    30
}

impl Default for RemoteProofingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            exposure: RemoteProofingExposure::default(),
            capture: RemoteProofingCapture::default(),
            encoder: RemoteProofingEncoder::default(),
            native_pairing_prompt: true,
            require_local_approval: true,
            show_shadowing_indicator: true,
            allow_remote_input: true,
            vnc_fallback: true,
            min_fps_target: default_min_fps(),
        }
    }
}

impl RemoteProofingConfig {
    /// The default Remote Proofing path (`<client-data-dir>/...json`), or `None`
    /// when no data dir resolves.
    fn default_path() -> Option<PathBuf> {
        mde_bus::client_data_dir().map(|d| d.join(REMOTE_PROOFING_CONFIG_FILE))
    }

    fn normalized(mut self) -> Self {
        self.min_fps_target = self.min_fps_target.clamp(15, 120);
        self
    }

    /// Build the render-free Sunshine/Moonlight service plan for the current mesh
    /// facts. The UI also renders this plan, so operator-visible exposure state and
    /// the future service bridge use the same source of truth.
    fn service_plan(self, mesh: &MeshFacts) -> RemoteProofingServicePlan {
        let cfg = self.normalized();
        let mut warnings = Vec::new();
        let (bind_scope, bind_address, firewall) = if !cfg.enabled {
            (
                RemoteProofingBindScope::Disabled,
                None,
                RemoteProofingFirewallPolicy::Closed,
            )
        } else {
            match cfg.exposure {
                RemoteProofingExposure::MeshOnly => {
                    if mesh.overlay_ip.is_none() {
                        warnings.push(
                            "Mesh address is not visible yet; keep the service degraded until the overlay address is known.",
                        );
                    }
                    (
                        RemoteProofingBindScope::MeshOnly,
                        mesh.overlay_ip.clone(),
                        RemoteProofingFirewallPolicy::MeshOverlayOnly,
                    )
                }
                RemoteProofingExposure::Lan => {
                    if mesh.default_gw.is_none() {
                        warnings.push(
                            "LAN exposure needs a trusted local interface before the service starts.",
                        );
                    }
                    (
                        RemoteProofingBindScope::Lan,
                        None,
                        RemoteProofingFirewallPolicy::TrustedLanOnly,
                    )
                }
                RemoteProofingExposure::Public => {
                    warnings.push(
                        "All-interfaces exposure must keep the firewall warning, local approval, and on-seat indicator visible.",
                    );
                    (
                        RemoteProofingBindScope::Public,
                        Some("0.0.0.0".to_owned()),
                        RemoteProofingFirewallPolicy::PublicExplicit,
                    )
                }
            }
        };

        if cfg.enabled && !cfg.require_local_approval {
            warnings.push("Local approval is off; use only for controlled proofing.");
        }
        if cfg.enabled && !cfg.show_shadowing_indicator {
            warnings.push("The on-seat shadowing indicator is off; remote viewers may be hidden.");
        }
        if cfg.enabled && !cfg.allow_remote_input {
            warnings.push("Remote viewers can watch only; keyboard and mouse input are blocked.");
        }

        RemoteProofingServicePlan {
            enabled: cfg.enabled,
            bind_scope,
            bind_address,
            firewall,
            sunshine_capture: cfg.capture.sunshine_value(),
            sunshine_encoder: cfg.encoder.sunshine_value(),
            min_fps_target: cfg.min_fps_target,
            native_pairing_prompt: cfg.native_pairing_prompt,
            require_local_approval: cfg.require_local_approval,
            show_shadowing_indicator: cfg.show_shadowing_indicator,
            allow_remote_input: cfg.allow_remote_input,
            vnc_fallback: cfg.vnc_fallback,
            warnings,
        }
    }

    /// Load from `path`, folding missing / malformed data to conservative defaults.
    fn load_from(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Self>(&s).ok())
            .map_or_else(Self::default, Self::normalized)
    }

    /// Load from the default path.
    #[must_use]
    fn load() -> Self {
        Self::default_path().map_or_else(Self::default, |p| Self::load_from(&p))
    }

    /// Write to `path` atomically, mirroring [`SettingsNav`] and
    /// [`AppearanceConfig`].
    ///
    /// # Errors
    /// The [`std::io::Error`] if the dir cannot be created or the file cannot be
    /// written / renamed.
    fn save_to(self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&self.normalized())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Persist to the default path (silent no-op when no data dir resolves).
    fn save(self) {
        if let Some(path) = Self::default_path() {
            let _ = self.save_to(&path);
        }
    }
}

// ──────────────────────────── Personalization → Theme (SETTINGS-5) ────────────────────────────

/// Runtime colour mode persisted by Personalization → Theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AppearanceColorScheme {
    /// Preserve the current dark platform look.
    #[default]
    Dark,
    /// Windows 2000 basic-inspired light look.
    Light,
}

impl AppearanceColorScheme {
    /// The visible picker order.
    const ALL: [Self; 2] = [Self::Dark, Self::Light];

    /// Picker label.
    const fn label(self) -> &'static str {
        match self {
            Self::Dark => "Dark",
            Self::Light => "Light",
        }
    }

    /// Short operator-facing description for the tile body.
    const fn description(self) -> &'static str {
        match self {
            Self::Dark => "Current platform colours",
            Self::Light => "Windows 2000 basic",
        }
    }

    /// Runtime mode consumed by `mde_egui::Style`.
    const fn runtime(self) -> StyleColorScheme {
        match self {
            Self::Dark => StyleColorScheme::Dark,
            Self::Light => StyleColorScheme::Light,
        }
    }
}

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

/// Runtime motion mode persisted by Personalization → Theme (MOTION-DRM-5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AppearanceMotionMode {
    /// Full shared motion.
    #[default]
    Normal,
    /// Vestibular-comfort mode: typed shared motion uses reduced preset durations,
    /// while older egui/boolean call sites are damped through `animation_time = 0`.
    Reduced,
    /// Endpoint-only mode.
    Disabled,
}

impl AppearanceMotionMode {
    /// The visible picker order.
    const ALL: [Self; 3] = [Self::Normal, Self::Reduced, Self::Disabled];

    /// Picker label.
    const fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Reduced => "Reduced",
            Self::Disabled => "Disabled",
        }
    }

    /// Short operator-facing description for the tile body.
    const fn description(self) -> &'static str {
        match self {
            Self::Normal => "Full transitions",
            Self::Reduced => "Short, calm motion",
            Self::Disabled => "Endpoint only",
        }
    }

    /// Runtime mode consumed by `mde_egui::Motion`.
    const fn runtime(self) -> MotionMode {
        match self {
            Self::Normal => MotionMode::Normal,
            Self::Reduced => MotionMode::Reduced,
            Self::Disabled => MotionMode::Disabled,
        }
    }
}

/// The client-data-dir file the Personalization → Theme appearance persists to (the
/// [`SettingsNav`] / `PowerHonorConfig` one-JSON-per-preference idiom).
const APPEARANCE_CONFIG_FILE: &str = "settings-appearance.json";

/// The persisted Personalization → Theme appearance (SETTINGS-5): the colour mode,
/// interactive accent, platform text-scale, and motion mode the shell actually
/// applies at runtime. Loaded on start and restored on open, saved on a pick — the
/// [`SettingsNav`] client-data-dir JSON idiom, reused. All fields drive a real live
/// effect through [`SystemState::apply_appearance`] or the shell's
/// `DockState` mirror (§7 — no dead toggle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
struct AppearanceConfig {
    /// The platform colour mode.
    #[serde(default)]
    color_scheme: AppearanceColorScheme,
    /// The interactive accent tint (re-applied over the installed look each frame).
    #[serde(default)]
    accent: AccentChoice,
    /// The whole-UI text-scale step (the EXPLORER-18 accessibility zoom).
    #[serde(default)]
    text_scale: TextScale,
    /// Runtime motion policy (MOTION-DRM-5): normal, reduced, or disabled. Driven live
    /// through [`SystemState::apply_appearance`], which flips the shared
    /// [`Motion::set_mode`] global and damps egui's legacy `animation_time` signal
    /// outside Normal.
    #[serde(default)]
    motion_mode: AppearanceMotionMode,
    /// WIN10-HYBRID taskbar auto-hide: when enabled, the bottom bar reserves no
    /// strut and reveals from the bottom hot edge.
    #[serde(default)]
    taskbar_autohide: bool,
}

impl<'de> Deserialize<'de> for AppearanceConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            #[serde(default)]
            color_scheme: AppearanceColorScheme,
            #[serde(default)]
            accent: AccentChoice,
            #[serde(default)]
            text_scale: TextScale,
            #[serde(default)]
            motion_mode: Option<AppearanceMotionMode>,
            #[serde(default)]
            taskbar_autohide: bool,
            #[serde(default)]
            reduce_motion: bool,
        }

        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            color_scheme: wire.color_scheme,
            accent: wire.accent,
            text_scale: wire.text_scale,
            motion_mode: wire.motion_mode.unwrap_or(if wire.reduce_motion {
                AppearanceMotionMode::Reduced
            } else {
                AppearanceMotionMode::Normal
            }),
            taskbar_autohide: wire.taskbar_autohide,
        })
    }
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
                    if settings_section_row(ui, section, nav.section == section) {
                        *nav = SettingsNav::at(section);
                    }
                }
            }
        });
}

fn settings_section_row(ui: &mut egui::Ui, section: SettingsSection, selected: bool) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        let accent = section.group().accent();
        let icon_tint = if selected { accent } else { Style::TEXT_DIM };
        let (icon_rect, icon_response) =
            ui.allocate_exact_size(egui::vec2(Style::SP_M, Style::SP_L), egui::Sense::click());
        let draw_rect =
            egui::Rect::from_center_size(icon_rect.center(), egui::vec2(Style::SP_M, Style::SP_M));
        if let Some(tex) = icon_texture(ui.ctx(), section.icon_id(), Style::SP_M, icon_tint) {
            egui::Image::new(egui::load::SizedTexture::new(tex.id(), draw_rect.size()))
                .paint_at(ui, draw_rect);
        }
        clicked |= icon_response.clicked();

        let text_tint = if selected { accent } else { Style::TEXT };
        let row = ui.add_sized(
            [ui.available_width(), Style::SP_L],
            egui::SelectableLabel::new(
                selected,
                RichText::new(section.label())
                    .size(Style::BODY)
                    .color(text_tint),
            ),
        );
        clicked |= row.clicked();
    });
    clicked
}

fn settings_detail_header(ui: &mut egui::Ui, section: SettingsSection) {
    let accent = section.group().accent();
    ui.horizontal(|ui| {
        if let Some(tex) = icon_texture(ui.ctx(), section.icon_id(), Style::SP_L, accent) {
            ui.add(egui::Image::new(egui::load::SizedTexture::new(
                tex.id(),
                egui::vec2(Style::SP_L, Style::SP_L),
            )));
        }
        ui.label(
            RichText::new(section.label())
                .color(accent)
                .size(Style::HEADING)
                .strong(),
        );
    });
}

fn settings_icon_button(ui: &mut egui::Ui, icon: IconId, tip: &str) -> egui::Response {
    let response = ui.add_sized(
        [Style::SP_L, Style::SP_L],
        egui::Button::new(RichText::new("")),
    );
    if let Some(tex) = icon_texture(ui.ctx(), icon, Style::SP_M, Style::TEXT) {
        let icon_rect = egui::Rect::from_center_size(
            response.rect.center(),
            egui::vec2(Style::SP_M, Style::SP_M),
        );
        egui::Image::new(egui::load::SizedTexture::new(tex.id(), icon_rect.size()))
            .paint_at(ui, icon_rect);
    }
    settings_hover_text(response, tip)
}

fn settings_tooltip(ui: &mut egui::Ui, text: &str) {
    egui::Frame::NONE
        .fill(Style::SURFACE)
        .stroke(egui::Stroke::new(1.0, Style::BORDER))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.set_max_width(SETTINGS_TOOLTIP_W);
            ui.add(
                egui::Label::new(RichText::new(text).size(Style::SMALL).color(Style::TEXT)).wrap(),
            );
        });
}

fn settings_hover_text(response: egui::Response, text: impl Into<String>) -> egui::Response {
    let text = text.into();
    response.on_hover_ui(move |ui| settings_tooltip(ui, text.as_str()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SettingsChoiceColors {
    fill: egui::Color32,
    stroke: egui::Color32,
    text: egui::Color32,
}

fn settings_choice_colors(
    ctx: &egui::Context,
    selected: bool,
    hovered: bool,
    accent: egui::Color32,
) -> SettingsChoiceColors {
    let scheme = Style::color_scheme(ctx);
    let palette = Style::current_palette(ctx);
    let accent = Style::accent_for_scheme(scheme, accent);
    SettingsChoiceColors {
        fill: if selected {
            Style::pressed_fill_for_scheme(scheme, accent)
        } else if hovered {
            palette.surface_hi
        } else {
            palette.surface
        },
        stroke: if selected || hovered {
            accent
        } else {
            palette.border
        },
        text: if selected {
            palette.text_strong
        } else {
            palette.text
        },
    }
}

fn settings_choice_button(
    ui: &mut egui::Ui,
    selected: bool,
    label: &str,
    accent: egui::Color32,
    height: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    if ui.is_rect_visible(rect) {
        let colors = settings_choice_colors(ui.ctx(), selected, response.hovered(), accent);
        ui.painter().rect(
            rect,
            Style::RADIUS,
            colors.fill,
            egui::Stroke::new(1.0, colors.stroke),
            egui::StrokeKind::Inside,
        );
        let text_rect = rect.shrink2(egui::vec2(Style::SP_S, 0.0));
        if text_rect.is_positive() {
            ui.painter().with_clip_rect(text_rect).text(
                text_rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(Style::BODY),
                colors.text,
            );
        }
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), selected, label)
    });
    response
}

fn settings_choice_tile(
    ui: &mut egui::Ui,
    selected: bool,
    label: &str,
    description: Option<&str>,
    accent: egui::Color32,
    height: f32,
) -> bool {
    let mut clicked = false;
    tile(ui, |ui| {
        if settings_choice_button(ui, selected, label, accent, height).clicked() && !selected {
            clicked = true;
        }
        if let Some(description) = description {
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new(description)
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
        }
    });
    clicked
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
    remote_proofing: &mut RemoteProofingConfig,
    mouse_touch: &mut MouseTouchConfig,
    appearance: &mut AppearanceConfig,
    agent_active: bool,
    prompt_in_flight: bool,
    actions: &mut Vec<SysAction>,
) {
    // Expressive header — the active section's title in the large type scale, tinted
    // in its domain group's categorical accent (SETTINGS-2) so the active domain reads
    // at a glance in the same colour as its rail header.
    settings_detail_header(ui, section);
    ui.add_space(Style::SP_M);
    // The section body sits on a Carbon layer-02 card raised above the layer-01 page,
    // ringed by a hairline border (SETTINGS-2 — [`section_card`]).
    section_card(ui, |ui| match section {
        SettingsSection::Displays => {
            displays_section(ui, snap, layout, panel_brightness, ddc_brightness, actions)
        }
        SettingsSection::Mouse => mouse_touch_section(ui, mouse_touch),
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
        SettingsSection::RemoteProofing => remote_proofing_section(ui, remote_proofing, mesh),
    });
}

/// The Settings **page** frame (SETTINGS-2) — Carbon **layer-01**: the rail + the
/// detail pane rest one elevation step above the window [`Style::BG`], the base the
/// section cards raise from. `margin` is the pane's inner pad (its own expressive
/// value per pane). All tokens — no raw literal (§4).
fn page_frame(margin: f32) -> egui::Frame {
    egui::Frame::NONE.fill(Style::LAYER_01).inner_margin(margin)
}

/// Build an [`egui::Shadow`] from the shared [`Elevation`] depth token — the surface-
/// side conversion the token module defers (it stays free of egui's shadow type). Reads
/// the token's offset/blur/spread/umbra, casting the logical-px floats onto epaint's
/// small integer fields; mints **no** colour of its own (the umbra comes straight from
/// the token), so the look still reads only from `mde_egui` (§4).
fn elevation_shadow(elevation: Elevation) -> egui::Shadow {
    let token = elevation.shadow();
    egui::Shadow {
        offset: [token.offset[0] as i8, token.offset[1] as i8],
        blur: token.blur as u8,
        spread: token.spread as u8,
        color: token.umbra,
    }
}

/// The Settings **section card** frame (SETTINGS-2) — Carbon **layer-02**: the
/// selected section's body sits one elevation step above the layer-01 page, ringed by
/// a hairline [`Style::BORDER`] with the shared corner radius, and casts the shared
/// [`Elevation::Raised`] soft shadow so it reads as genuinely lifted off the page (a
/// translucent depth, lock #2). Every value is a token — fill / stroke / radius / pad /
/// shadow (no raw literal, §4).
fn card_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(Style::LAYER_02)
        .stroke(egui::Stroke::new(1.0, Style::BORDER))
        .corner_radius(Style::RADIUS)
        .inner_margin(Style::SP_M)
        .shadow(elevation_shadow(Elevation::Raised))
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
            muted_note(ui, SYSTEM_READING_SEAT_COPY);
        }
        Some(Probe::Present(v)) => present(ui, v),
        Some(Probe::Absent { reason, .. }) => {
            muted_note(ui, reason.clone());
        }
    }
}

/// Devices → Mouse & Touch: pointer sensitivity, handedness, scroll, double-click,
/// touchpad tap/two-finger controls, and Surface-class touchscreen gestures. The
/// native DRM effects are published from [`SystemState::apply_mouse_touch`]; the
/// touchpad tap policy also mirrors the `mackesd` setting key for compositor seats.
fn mouse_touch_section(ui: &mut egui::Ui, config: &mut MouseTouchConfig) {
    ui.columns(2, |columns| {
        column_card(&mut columns[0], "Pointer", |ui| {
            let mut speed = config.pointer_speed_percent;
            if ui
                .add(
                    Slider::new(&mut speed, -100..=100)
                        .text("Pointer speed")
                        .suffix("%"),
                )
                .changed()
            {
                config.pointer_speed_percent = speed;
            }
            ui.label(
                RichText::new(format!(
                    "mackesd mouse.pointer_accel {:+.2}",
                    config.mackesd_pointer_accel()
                ))
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
            ui.add_space(Style::SP_S);

            ComboBox::from_id_salt(ui.id().with("primary-button"))
                .selected_text(config.primary_button.label())
                .show_ui(ui, |ui| {
                    for choice in PrimaryButton::ALL {
                        ui.selectable_value(&mut config.primary_button, choice, choice.label());
                    }
                });
        });

        column_card(&mut columns[1], "Scroll & Click", |ui| {
            let mut natural_scroll = config.natural_scroll;
            if ui
                .checkbox(
                    &mut natural_scroll,
                    RichText::new("Natural scrolling").size(Style::BODY),
                )
                .changed()
            {
                config.natural_scroll = natural_scroll;
            }

            let mut scroll_speed = i32::from(config.scroll_speed_percent);
            if ui
                .add(
                    Slider::new(&mut scroll_speed, 25..=300)
                        .text("Scroll speed")
                        .suffix("%"),
                )
                .changed()
            {
                config.scroll_speed_percent = scroll_speed.clamp(25, 300) as u16;
            }

            let mut double_click = i32::from(config.double_click_ms);
            if ui
                .add(
                    Slider::new(&mut double_click, 150..=900)
                        .text("Double-click")
                        .suffix(" ms"),
                )
                .changed()
            {
                config.double_click_ms = double_click.clamp(150, 900) as u16;
            }
        });
    });

    ui.add_space(Style::SP_M);
    ui.columns(2, |columns| {
        column_card(&mut columns[0], "Touchpad", |ui| {
            let mut tap_to_click = config.touchpad_tap_to_click;
            if ui
                .checkbox(
                    &mut tap_to_click,
                    RichText::new("Tap to click").size(Style::BODY),
                )
                .changed()
            {
                config.touchpad_tap_to_click = tap_to_click;
            }

            let mut two_finger_scroll = config.two_finger_scroll;
            if ui
                .checkbox(
                    &mut two_finger_scroll,
                    RichText::new("Two-finger scroll").size(Style::BODY),
                )
                .changed()
            {
                config.two_finger_scroll = two_finger_scroll;
            }
        });

        column_card(&mut columns[1], "Touch & Surface", |ui| {
            let mut touchscreen_enabled = config.touchscreen_enabled;
            if ui
                .checkbox(
                    &mut touchscreen_enabled,
                    RichText::new("Touchscreen input").size(Style::BODY),
                )
                .changed()
            {
                config.touchscreen_enabled = touchscreen_enabled;
            }

            let mut edge_gestures = config.edge_gestures;
            if ui
                .checkbox(
                    &mut edge_gestures,
                    RichText::new("Edge gestures").size(Style::BODY),
                )
                .changed()
            {
                config.edge_gestures = edge_gestures;
            }

            let mut long_press_secondary = config.long_press_secondary;
            if ui
                .checkbox(
                    &mut long_press_secondary,
                    RichText::new("Long-press secondary click").size(Style::BODY),
                )
                .changed()
            {
                config.long_press_secondary = long_press_secondary;
            }
        });
    });

    ui.add_space(Style::SP_S);
    muted_note(
        ui,
        "Pointer speed, handedness, wheel direction, scroll speed, double-click timing, \
         touchscreen input, edge gestures, and long-press secondary click are applied \
         by the native seat. Tap-to-click mirrors the existing compositor input policy.",
    );
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
                    RichText::new(SYSTEM_SCANNING_COPY).size(Style::SMALL),
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
                    if settings_icon_button(ui, DISPLAY_NUDGE_LEFT_ICON, "Move display left")
                        .clicked()
                    {
                        actions.push(SysAction::Nudge(out.id.clone(), true));
                    }
                    if settings_icon_button(ui, DISPLAY_NUDGE_RIGHT_ICON, "Move display right")
                        .clicked()
                    {
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
/// official Construct wallpapers (placement lock #12). The choice persists per seat and
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
        if settings_choice_tile(
            ui,
            selected,
            wallpaper.label(),
            None,
            SettingsGroup::Personalization.accent(),
            Style::SP_XL,
        ) {
            crate::backdrop::select_wallpaper(&ctx, wallpaper);
        }
    });
    ui.add_space(Style::SP_S);
    muted_note(
        ui,
        "The five official Construct wallpapers ship in the RPM; your choice follows your mesh identity when a workgroup volume is present.",
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

/// The Theme section (SETTINGS-5) under Personalization — the appearance controls
/// the shell **genuinely applies at runtime**: the **colour mode** (dark status quo
/// or Windows 2000 basic light), the interactive **accent**, the **text-scale**,
/// the **motion mode** (MOTION-DRM-5 normal/reduced/disabled runtime policy), and
/// the Win10-hybrid taskbar auto-hide preference mirrored into `DockState`.
/// Each pick mutates the persisted [`AppearanceConfig`] in place; the change is
/// saved after the render borrow and applied live by [`SystemState::apply_appearance`]
/// / `main.rs` on the next frame. Laid **across** the wide pane (SETTINGS-3) as
/// selectable tiles.
fn theme_section(ui: &mut egui::Ui, appearance: &mut AppearanceConfig) {
    // Colour mode — a real runtime palette selector. Dark preserves the shipped
    // dark look; Light uses classic Windows 2000 basic system colours.
    ui.label(
        RichText::new("Mode")
            .color(Style::TEXT_DIM)
            .size(Style::SMALL)
            .strong(),
    );
    ui.add_space(Style::SP_XS);
    across_grid(ui, &AppearanceColorScheme::ALL, 2, |ui, &scheme| {
        let selected = appearance.color_scheme == scheme;
        if settings_choice_tile(
            ui,
            selected,
            scheme.label(),
            Some(scheme.description()),
            SettingsGroup::Personalization.accent(),
            Style::SP_XL,
        ) {
            appearance.color_scheme = scheme;
        }
    });
    ui.add_space(Style::SP_M);
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
                if settings_choice_button(
                    ui,
                    selected,
                    choice.label(),
                    SettingsGroup::Personalization.accent(),
                    Style::SP_L,
                )
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
        if settings_choice_tile(
            ui,
            selected,
            scale.label(),
            None,
            SettingsGroup::Personalization.accent(),
            Style::SP_XL,
        ) {
            appearance.text_scale = scale;
        }
    });
    ui.add_space(Style::SP_M);
    // Motion mode — the runtime normal/reduced/disabled policy for shared motion.
    ui.label(
        RichText::new("Motion")
            .color(Style::TEXT_DIM)
            .size(Style::SMALL)
            .strong(),
    );
    ui.add_space(Style::SP_XS);
    across_grid(ui, &AppearanceMotionMode::ALL, 3, |ui, &mode| {
        let selected = appearance.motion_mode == mode;
        if settings_choice_tile(
            ui,
            selected,
            mode.label(),
            Some(mode.description()),
            SettingsGroup::Personalization.accent(),
            Style::SP_XL,
        ) {
            appearance.motion_mode = mode;
        }
    });
    ui.add_space(Style::SP_M);
    ui.label(
        RichText::new("Taskbar")
            .color(Style::TEXT_DIM)
            .size(Style::SMALL)
            .strong(),
    );
    ui.add_space(Style::SP_XS);
    tile(ui, |ui| {
        let mut taskbar_autohide = appearance.taskbar_autohide;
        if ui
            .checkbox(
                &mut taskbar_autohide,
                RichText::new("Auto-hide taskbar").size(Style::BODY),
            )
            .changed()
        {
            appearance.taskbar_autohide = taskbar_autohide;
        }
        ui.label(
            RichText::new("Reveal from the bottom edge")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
    });
    ui.add_space(Style::SP_S);
    muted_note(
        ui,
        "Accent re-tints every surface's highlights; text size scales the whole \
         interface. Motion mode applies normal, reduced, or endpoint-only movement. \
         Taskbar auto-hide floats the bottom bar from the edge.",
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

mod mesh;
use mesh::*;

#[cfg(test)]
mod tests;
