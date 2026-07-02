//! `Surface::System` — this seat's host-controls panel (E12-15).
//!
//! Under E12 "Quasar" the shell owns the DRM seat with no compositor and no
//! settings daemon, so audio / Bluetooth / displays / power / backlight have no
//! owner until `mde-seat` (design `docs/design/quasar-host-controls.md`). This
//! surface is where ALL host-control interaction lives (lock 3); the chrome bar
//! keeps only read-only status icons (see [`crate::chrome`]).
//!
//! The one render model is [`mde_seat::SeatSnapshot`] — every section is a
//! [`Probe`]: `Present` shows the real rows, `Absent` shows the shared honest
//! "not available" note (§7 / interlock 4), never a fake control. E12-15 is
//! **status only**: the mixer faders, pairing verbs, mode/arrange, brightness
//! sliders and power buttons land in E12-16..E12-19 — this panel renders the
//! honest inventory those units then make interactive. The Hotkeys section
//! renders the fixed compiled-in table ([`HOTKEYS`], lock 9) read-only; its
//! dispatch is E12-19's work.
//!
//! The state holds the ONE [`Seat`] (lock 1) and re-`snapshot()`s it on the
//! shell's shared pump cadence; the same cached snapshot feeds the chrome icons,
//! so the panel and the chrome can't diverge.

use std::time::{Duration, Instant};

use mde_egui::egui::{self, RichText};
use mde_egui::{field, muted_note, Style};

use mde_seat::{
    Avail, BtStatus, ConnectorStatus, MixerStatus, MixerStrip, PowerCaps, Probe, Seat,
    SeatSnapshot, HOTKEYS,
};

/// Poll cadence — a device plug, a battery drain, or a BT connect surfaces within
/// this window. Matches the chrome bar + the Workbench planes; the snapshot is a
/// handful of cheap D-Bus/sysfs reads (each absent backend fails fast), so the
/// cadence can stay tight.
const REFRESH: Duration = Duration::from_secs(5);

/// A filled-circle status dot — the shared glyph the rest of the platform uses, so
/// a status dot reads one `Style` size + colour.
const DOT: &str = "\u{25CF}";

// ──────────────────────────── the System state ────────────────────────────

/// The System surface's live state: the ONE [`Seat`] (lock 1) plus its latest
/// snapshot, refreshed on the shared cadence. The cached snapshot is the single
/// model both this panel and the chrome status icons render from.
pub(crate) struct SystemState {
    /// The one seat over the real host hardware (in-process, lock 1).
    seat: Seat,
    /// The latest snapshot. `None` until the first poll — drives the honest
    /// "reading the seat…" state, never a blank panel.
    snapshot: Option<SeatSnapshot>,
    /// When the seat was last snapshotted (drives the fixed cadence).
    last_poll: Option<Instant>,
}

impl Default for SystemState {
    fn default() -> Self {
        Self {
            seat: Seat::new(),
            snapshot: None,
            last_poll: None,
        }
    }
}

impl SystemState {
    /// The poll seam: re-snapshot the seat when the cadence has elapsed, then keep
    /// the repaint heartbeat alive so a device plug / battery flip surfaces without
    /// input. `Seat::snapshot` is infallible as a whole (each section folds to a
    /// typed `Present`/`Absent`), so this never fails — cheap enough to call every
    /// frame; it self-gates.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            self.snapshot = Some(self.seat.snapshot());
        }
        ctx.request_repaint_after(REFRESH);
    }

    /// The latest seat snapshot, for the chrome status icons ([`crate::chrome`]).
    /// `None` until the first poll.
    pub(crate) const fn snapshot(&self) -> Option<&SeatSnapshot> {
        self.snapshot.as_ref()
    }

    /// Render the surface's live content into `ui`.
    pub(crate) fn show(&self, ui: &mut egui::Ui) {
        show_system(ui, self.snapshot.as_ref());
    }
}

// ──────────────────────────── render ────────────────────────────

/// Render the System surface: the six sections (Mixer / Bluetooth / Displays /
/// Power & Battery / Backlight / Hotkeys), each honest about its backend.
fn show_system(ui: &mut egui::Ui, snap: Option<&SeatSnapshot>) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            section(ui, "Mixer", |ui| mixer_section(ui, snap));
            section(ui, "Bluetooth", |ui| bluetooth_section(ui, snap));
            section(ui, "Displays", |ui| displays_section(ui, snap));
            section(ui, "Power & Battery", |ui| power_section(ui, snap));
            section(ui, "Backlight", |ui| backlight_section(ui, snap));
            section(ui, "Hotkeys", hotkeys_section);
        });
}

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

/// Fold one snapshot [`Probe`] section into its render: not-yet-polled →
/// "reading…", `Absent` → the shared honest not-available note (§7), `Present` →
/// the caller's rows. The one place the Present/Absent/loading branch lives.
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
            // The honest typed "not available on this host" line — never a fake
            // control (§7 / interlock 4).
            muted_note(ui, reason.clone());
        }
    }
}

/// The Mixer section — the master strip then every channel strip, read-only
/// (fader / mute / solo interaction lands in E12-16). `Absent` until the
/// `PipeWire` binding lands (E12-16) or when no `PipeWire` daemon runs.
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

/// One mixer strip as a read-only status row: name → level (+ a muted marker).
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

/// The Bluetooth section — adapters then known devices, read-only (pairing verbs
/// land in E12-17). `Absent` when `BlueZ` / the system bus is absent.
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

/// The Displays section — every DRM connector + its preferred mode, read-only
/// (enable / mode / arrange land in E12-18). `Absent` on a host with no `/dev/dri`.
fn displays_section(ui: &mut egui::Ui, snap: Option<&SeatSnapshot>) {
    probe_section(
        ui,
        snap,
        |s| &s.displays,
        |ui, connectors| {
            if connectors.is_empty() {
                muted_note(ui, "No connectors.");
            }
            for connector in connectors {
                let (word, tone) = match connector.status {
                    ConnectorStatus::Connected => ("connected", Style::OK),
                    ConnectorStatus::Disconnected => ("disconnected", Style::TEXT_DIM),
                    ConnectorStatus::Unknown => ("unknown", Style::TEXT_DIM),
                };
                let value = connector.preferred_mode().map_or_else(
                    || word.to_owned(),
                    |mode| format!("{word} \u{00B7} {}", mode.label()),
                );
                field(ui, &connector.name, &value, tone);
            }
        },
    );
}

/// The Power & Battery section — logind's confirm-gated power capabilities (read-
/// only availability; the buttons land in E12-19) then every `UPower` battery
/// (multi + peripherals, lock 6). Two independent probes; either can be `Absent`.
fn power_section(ui: &mut egui::Ui, snap: Option<&SeatSnapshot>) {
    probe_section(
        ui,
        snap,
        |s| &s.power,
        |ui, caps: &PowerCaps| {
            avail_row(ui, "Suspend", caps.suspend);
            avail_row(ui, "Reboot", caps.reboot);
            avail_row(ui, "Power off", caps.poweroff);
        },
    );
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
}

/// One power-verb availability row — an honest label (available / refused / needs
/// authorization / not supported), toned by whether it could ever succeed. Read-
/// only: the verb itself is confirm-gated in E12-19.
fn avail_row(ui: &mut egui::Ui, label: &str, avail: Avail) {
    let tone = if avail.offerable() {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };
    field(ui, label, avail.label(), tone);
}

/// The Backlight section — every sysfs panel + its brightness percentage, read-
/// only (the slider lands in E12-18). `Absent` on a host with no backlight class.
fn backlight_section(ui: &mut egui::Ui, snap: Option<&SeatSnapshot>) {
    probe_section(
        ui,
        snap,
        |s| &s.backlights,
        |ui, panels| {
            if panels.is_empty() {
                muted_note(ui, "No backlight panels.");
            }
            for panel in panels {
                field(
                    ui,
                    &panel.name,
                    &format!("{}%", panel.percent()),
                    Style::TEXT,
                );
            }
        },
    );
}

/// The Hotkeys section — the fixed compiled-in table (lock 9) rendered read-only:
/// chord → typed action label. Not snapshot-derived (the table is a compile-time
/// constant); dispatch is E12-19's work.
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

    /// Drive one headless 960×640 frame of `show_system` and tessellate it on the
    /// CPU — the same `Context::run` → `tessellate` path the DRM runner drives
    /// minus the GPU. Returns whether it produced any draw primitives.
    fn renders(snap: Option<&SeatSnapshot>) -> bool {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show_system(ui, snap));
        });
        !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty()
    }

    #[test]
    fn the_pre_poll_state_is_a_full_paint_not_a_blank_panel() {
        // Before the first snapshot every section reads "reading the seat…" — a
        // real paint path, plus the always-present read-only Hotkeys table.
        assert!(
            renders(None),
            "the pre-poll System panel produced no primitives"
        );
    }

    #[test]
    fn a_real_seat_snapshot_mounts_and_renders_every_section() {
        // The DoD render: over a REAL `Seat::snapshot()`. On the headless build
        // host most backends are legitimately Absent — each shows its honest typed
        // not-available line (§7), which is still a full paint path, never blank.
        let snap = Seat::new().snapshot();
        assert!(
            renders(Some(&snap)),
            "the live System panel produced no draw primitives"
        );
    }

    #[test]
    fn default_state_holds_a_seat_and_is_unpolled() {
        let st = SystemState::default();
        assert!(st.snapshot().is_none(), "no snapshot before the first poll");
        assert!(st.last_poll.is_none());
    }

    #[test]
    fn the_hotkeys_section_renders_the_whole_fixed_table_read_only() {
        // The read-only Hotkeys table is snapshot-independent — every compiled-in
        // chord is a paint path even with no seat hardware at all.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, hotkeys_section);
        });
        assert!(!ctx.tessellate(out.shapes, out.pixels_per_point).is_empty());
        assert!(!HOTKEYS.is_empty(), "the fixed hotkey table is non-empty");
    }
}
