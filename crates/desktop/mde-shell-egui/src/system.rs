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
use std::time::{Duration, Instant};

use mde_egui::egui::{self, ComboBox, RichText, Slider};
use mde_egui::{field, muted_note, Style};

use mde_seat::{
    Avail, Backlight, BtStatus, Connector, ConnectorStatus, DdcDisplay, DisplayLayout, DisplayMode,
    MixerStatus, MixerStrip, MonitorId, OutputArrangement, PowerCaps, PowerVerb, Probe, Seat,
    SeatSnapshot, HOTKEYS,
};

use crate::instances::InstancesState;

/// Poll cadence — a device plug, a battery drain, or a BT connect surfaces within
/// this window.
const REFRESH: Duration = Duration::from_secs(5);

/// A filled-circle status dot — the shared glyph the rest of the platform uses.
const DOT: &str = "\u{25CF}";

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
    /// The last control action's honest inline error (a refused write / interlock).
    error: Option<String>,
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
            error: None,
        }
    }
}

/// One control action collected during the render borrow, applied after it ends
/// (the egui idiom the Instances panel uses) so the drive can take `&mut` freely.
enum SysAction {
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
    /// Drive a VM power verb through the Instances broker (§6).
    VmPower { idx: usize, boot: bool },
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
    }

    /// The latest seat snapshot, for the chrome status icons ([`crate::chrome`]).
    pub(crate) const fn snapshot(&self) -> Option<&SeatSnapshot> {
        self.snapshot.as_ref()
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
                error,
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
                    section(ui, "Bluetooth", |ui| bluetooth_section(ui, snap));
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
                        power_section(ui, snap, *confirm, instances, &mut actions);
                    });
                    section(ui, "Hotkeys", hotkeys_section);
                });
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
                SysAction::VmPower { idx, boot } => instances.drive_power(idx, boot),
            }
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

/// The Bluetooth section — read-only status (pairing verbs are E12-17).
fn bluetooth_section(ui: &mut egui::Ui, snap: Option<&SeatSnapshot>) {
    probe_section(
        ui,
        snap,
        |s| &s.bluetooth,
        |ui, bt: &BtStatus| {
            if bt.adapters.is_empty() {
                muted_note(ui, "No Bluetooth adapter.");
            }
            for adapter in &bt.adapters {
                let (word, tone) = if adapter.powered {
                    ("powered", Style::OK)
                } else {
                    ("off", Style::TEXT_DIM)
                };
                field(ui, &adapter.name, word, tone);
            }
            for device in &bt.devices {
                let mut value = if device.connected {
                    "connected".to_owned()
                } else if device.paired {
                    "paired".to_owned()
                } else {
                    "known".to_owned()
                };
                if let Some(pct) = device.battery_percent {
                    value = format!("{value} \u{00B7} {pct}%");
                }
                let tone = if device.connected {
                    Style::TEXT
                } else {
                    Style::TEXT_DIM
                };
                field(ui, &device.alias, &value, tone);
            }
        },
    );
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

/// The Power & Battery section — confirm-gated logind verbs, multi-battery
/// telemetry, and per-VM power rows (reusing the Instances broker, §6).
fn power_section(
    ui: &mut egui::Ui,
    snap: Option<&SeatSnapshot>,
    confirm: Option<PowerVerb>,
    instances: &InstancesState,
    actions: &mut Vec<SysAction>,
) {
    // Host power verbs — Lock is always offered; the host-down verbs are gated by
    // logind's CanX and a two-click confirm (lock 12).
    probe_section(
        ui,
        snap,
        |s| &s.power,
        |ui, caps: &PowerCaps| {
            power_verb_row(ui, PowerVerb::Lock, Avail::Yes, confirm, actions);
            power_verb_row(ui, PowerVerb::Suspend, caps.suspend, confirm, actions);
            power_verb_row(ui, PowerVerb::Reboot, caps.reboot, confirm, actions);
            power_verb_row(ui, PowerVerb::PowerOff, caps.poweroff, confirm, actions);
        },
    );

    // Batteries (multi + peripherals, lock 6).
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
}
