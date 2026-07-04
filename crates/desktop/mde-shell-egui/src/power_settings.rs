//! POWER-4 — the Power Settings controls of the System surface's "Power &
//! Battery" section.
//!
//! The E12-18 Power section landed the confirm-gated logind verbs + read-only
//! battery telemetry; POWER-4 makes the rest of what the POWER-2/3 backend can
//! actually drive interactive, and *only* what it can drive — no inert control
//! (§7). Each body-builder here is a pure fold over a [`mde_seat::SeatSnapshot`]
//! [`Probe`](mde_seat::Probe) reading (already unwrapped `Present` by the
//! section's `probe_section`), emitting the same [`SysAction`]s the section's
//! `apply()` drives against the ONE seat (lock 1):
//!
//! - **Power profile** — a segmented control over the daemon's advertised set
//!   ([`profile_body`]); selecting one emits [`SysAction::SetPowerProfile`]. The
//!   Absent case (power-profiles-daemon not running) is the probe's honest
//!   "unavailable" reason, never a fabricated active.
//! - **On-AC / on-battery** — the honest `UPower` `LinePower` reading
//!   ([`ac_source_body`]): "On AC power" / "On battery" / "Power source unknown".
//! - **Charge limit** — the charge-stop cap slider ([`charge_threshold_body`])
//!   when a battery advertises `charge_control_end_threshold`; an honest "not
//!   supported" when `Present(None)`; a refused/EACCES write is surfaced typed by
//!   the section (via [`charge_error`]), never a pretend cap.
//! - **Rich telemetry** — time-to-empty / time-to-full + draw rate
//!   ([`battery_telemetry`]), formatted compactly, omitted honestly when absent.
//!
//! POWER-5 adds two more, now non-inert: the **idle** picker — timeout + action
//! ([`idle_timeout_body`]) — and the **lid-close action** dropdown
//! ([`lid_action_body`]). These edit the persisted [`PowerHonorConfig`] the
//! [`crate::power_honor`] honorer enforces every frame — so they are §7-real
//! (the timer really acts on idle, the lid handler really acts), never a dead
//! control. The safe defaults live in the config: idle **Never**, lid **Suspend**.
//! CURTAIN-3 makes the idle/lid **action** selectable — Suspend / **Lock** (drop the
//! shell's curtain in-process) / Do nothing.
//!
//! Token-only styling (§4): every colour/size/space is a [`Style`] constant.

#![allow(
    clippy::redundant_pub_crate,
    reason = "pub(crate) body-builders in a private shell module are this crate's \
              idiom (curtain, dock, tray, …); the System surface's Power section \
              consumes them"
)]

use std::time::Duration;

use mde_egui::egui::{self, ComboBox, RichText, Slider};
use mde_egui::{field, muted_note, Style};
use mde_seat::{Battery, ProfileState, SeatError};

use crate::power_honor::{IdleAction, LidAction, PowerHonorConfig};
use crate::system::SysAction;

/// The charge-stop cap slider's range. Below ~50% is rarely a useful pack-sparing
/// cap, and 100 is "charge fully" — the same window the vendor tools expose.
const CHARGE_MIN: u8 = 50;
/// The upper bound of the charge-stop cap slider (charge fully).
const CHARGE_MAX: u8 = 100;

// ──────────────────────────── power profile ────────────────────────────

/// Prettify a power-profile id for display. Known freedesktop names get a proper
/// case; an unknown profile is shown verbatim (honest, never guessed — §7).
#[must_use]
pub(crate) fn profile_label(name: &str) -> String {
    match name {
        "power-saver" => "Power-saver".to_owned(),
        "balanced" => "Balanced".to_owned(),
        "performance" => "Performance".to_owned(),
        other => other.to_owned(),
    }
}

/// The action a profile selection dispatches: `Some(SetPowerProfile)` only when
/// `name` is one the daemon actually offers AND is not already active — so an
/// unknown name is never sent and the active profile is never re-dispatched
/// (§7: no inert re-send).
#[must_use]
pub(crate) fn profile_action(state: &ProfileState, name: &str) -> Option<SysAction> {
    (state.offers(name) && state.active != name)
        .then(|| SysAction::SetPowerProfile(name.to_owned()))
}

/// The power-profile body: a segmented control over the daemon's `available`
/// set, the active one highlighted; selecting another emits
/// [`SysAction::SetPowerProfile`]. A `Present` state with no profiles is an
/// honest empty note (distinct from the section being Absent).
pub(crate) fn profile_body(ui: &mut egui::Ui, state: &ProfileState, actions: &mut Vec<SysAction>) {
    if state.available.is_empty() {
        muted_note(ui, "Power profile: the daemon offered no profiles.");
        return;
    }
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Power profile")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);
        for name in &state.available {
            let selected = state.active == *name;
            if ui
                .selectable_label(selected, RichText::new(profile_label(name)).size(Style::SMALL))
                .clicked()
            {
                if let Some(action) = profile_action(state, name) {
                    actions.push(action);
                }
            }
        }
    });
}

// ──────────────────────────── power source (on-AC) ────────────────────────────

/// The honest on-AC / on-battery / unknown label for the `UPower` `LinePower`
/// reading. `None` is the honest "no adapter tracked" (a desktop), never guessed.
#[must_use]
pub(crate) const fn ac_source_label(on_ac: Option<bool>) -> &'static str {
    match on_ac {
        Some(true) => "On AC power",
        Some(false) => "On battery",
        None => "Power source unknown",
    }
}

/// The on-AC / on-battery source line: a toned [`field`] row — OK on AC, WARN on
/// battery, dim when unknown.
pub(crate) fn ac_source_body(ui: &mut egui::Ui, on_ac: Option<bool>) {
    let tone = match on_ac {
        Some(true) => Style::OK,
        Some(false) => Style::WARN,
        None => Style::TEXT_DIM,
    };
    field(ui, "Power source", ac_source_label(on_ac), tone);
}

// ──────────────────────────── charge threshold ────────────────────────────

/// The charge-limit body: the charge-stop cap slider when a battery advertises
/// the attribute (`cap` is `Present(Some)`), seeded into the caller-owned `live`
/// value so a drag stays smooth; an honest "not supported on this machine" when
/// `Present(None)` (the class exists but no battery advertises it). The `Absent`
/// case (no power-supply class at all) is the section's probe reason, never a
/// dead slider (§7).
pub(crate) fn charge_threshold_body(
    ui: &mut egui::Ui,
    cap: Option<u8>,
    live: &mut Option<u8>,
    actions: &mut Vec<SysAction>,
) {
    let Some(seed) = cap else {
        muted_note(ui, "Charge limit: not supported on this machine.");
        return;
    };
    let val = live.get_or_insert(seed);
    if ui
        .add(
            Slider::new(val, CHARGE_MIN..=CHARGE_MAX)
                .suffix("%")
                .text(RichText::new("Charge limit").size(Style::SMALL)),
        )
        .changed()
    {
        actions.push(SysAction::SetChargeThreshold(*val));
    }
    muted_note(
        ui,
        "Caps charging below full to spare the pack; the write needs a privileged session.",
    );
}

/// Fold a charge-threshold write failure into the section's honest inline error
/// (§7): the typed [`SeatError`] — `Unavailable` (no advertising battery),
/// `OutOfRange`, or the EACCES `Io` on the root-owned sysfs attribute — is
/// surfaced verbatim, never a pretend success.
#[must_use]
pub(crate) fn charge_error(e: &SeatError) -> String {
    format!("Charge limit: {e}")
}

/// Fold a power-profile switch failure into the section's honest inline error:
/// the typed `Unavailable` (no daemon) / `Backend` (refused name) is surfaced,
/// never a silent no-op (§7).
#[must_use]
pub(crate) fn profile_error(e: &SeatError) -> String {
    format!("Power profile: {e}")
}

// ──────────────────────────── rich telemetry ────────────────────────────

/// Format a `UPower` ETA [`Duration`] as a compact `~2h 14m` / `~45m` / `~<1m`.
#[must_use]
pub(crate) fn format_duration(d: Duration) -> String {
    let mins_total = d.as_secs() / 60;
    let (h, m) = (mins_total / 60, mins_total % 60);
    if h > 0 {
        format!("~{h}h {m}m")
    } else if mins_total > 0 {
        format!("~{m}m")
    } else {
        "~<1m".to_owned()
    }
}

/// Format an `EnergyRate` as `11.7 W`.
#[must_use]
pub(crate) fn format_energy_rate(watts: f64) -> String {
    format!("{watts:.1} W")
}

/// The battery's rich telemetry line — time-to-empty / time-to-full + draw rate,
/// joined with a middot — or `None` when `UPower` reported none of them (an honest
/// omission, never a fabricated ETA or 0 W draw — §7). The backend already folds
/// a `UPower` "0" ("no estimate") to `None`, so only real readings appear here.
#[must_use]
pub(crate) fn battery_telemetry(b: &Battery) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(d) = b.time_to_empty {
        parts.push(format!("{} to empty", format_duration(d)));
    }
    if let Some(d) = b.time_to_full {
        parts.push(format!("{} to full", format_duration(d)));
    }
    if let Some(w) = b.energy_rate {
        parts.push(format_energy_rate(w));
    }
    (!parts.is_empty()).then(|| parts.join("  \u{00B7}  "))
}

// ──────────────────────────── idle-suspend + lid (POWER-5) ────────────────────────────

/// The idle-suspend timeout options, in picker order. `None` = Never (off) — the
/// SAFE DEFAULT so a fresh install never surprise-suspends until the operator arms
/// it; the rest are the familiar 1 / 5 / 10 / 30-minute steps.
const IDLE_OPTIONS: [Option<u64>; 5] = [None, Some(1), Some(5), Some(10), Some(30)];

/// The operator-facing label for an idle-timeout option.
#[must_use]
pub(crate) fn idle_option_label(mins: Option<u64>) -> String {
    mins.map_or_else(|| "Never".to_owned(), |m| format!("{m} min"))
}

/// The idle picker (POWER-5 + CURTAIN-3) — the idle **timeout** (Never / 1 / 5 / 10 /
/// 30 min) plus what firing it **does** (Suspend / **Lock** / Do nothing), both editing
/// the honorer's persisted [`PowerHonorConfig`]. A real change writes the new value into
/// `config` and dispatches [`SysAction::SavePowerHonorConfig`] so it persists; an
/// unchanged re-pick is not re-saved (§7: no inert write). The honorer reads this config
/// every frame, so both choices are enforced — never a dead control. CURTAIN-3's Lock
/// action drops the shell's in-process curtain when the timeout fires.
pub(crate) fn idle_timeout_body(
    ui: &mut egui::Ui,
    config: &mut PowerHonorConfig,
    actions: &mut Vec<SysAction>,
) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Idle timeout")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);
        ComboBox::from_id_salt("power5-idle-timeout")
            .selected_text(
                RichText::new(idle_option_label(config.idle_timeout_min)).size(Style::SMALL),
            )
            .show_ui(ui, |ui| {
                for opt in IDLE_OPTIONS {
                    let selected = config.idle_timeout_min == opt;
                    if ui
                        .selectable_label(
                            selected,
                            RichText::new(idle_option_label(opt)).size(Style::SMALL),
                        )
                        .clicked()
                        && !selected
                    {
                        config.idle_timeout_min = opt;
                        actions.push(SysAction::SavePowerHonorConfig);
                    }
                }
            });
    });
    // CURTAIN-3 — what firing the idle timeout does: Suspend (default) / Lock (drop
    // the curtain in-process) / Do nothing. Mirrors the lid-action picker; the honorer
    // routes a Lock to the curtain, never to logind.
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("When idle")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);
        ComboBox::from_id_salt("curtain3-idle-action")
            .selected_text(RichText::new(config.idle_action.label()).size(Style::SMALL))
            .show_ui(ui, |ui| {
                for action in IdleAction::ALL {
                    let selected = config.idle_action == action;
                    if ui
                        .selectable_label(selected, RichText::new(action.label()).size(Style::SMALL))
                        .clicked()
                        && !selected
                    {
                        config.idle_action = action;
                        actions.push(SysAction::SavePowerHonorConfig);
                    }
                }
            });
    });
    muted_note(
        ui,
        "Runs the chosen action after the idle time; \u{201C}Never\u{201D} keeps the seat awake.",
    );
}

/// The lid-close action dropdown (POWER-5) — Suspend / Lock / Do nothing, editing
/// the honorer's persisted [`PowerHonorConfig`]. Suspend is the default. A real
/// change writes the choice and dispatches [`SysAction::SavePowerHonorConfig`]; the
/// honorer acts on the next Open→Closed lid edge, so this is §7-real.
pub(crate) fn lid_action_body(
    ui: &mut egui::Ui,
    config: &mut PowerHonorConfig,
    actions: &mut Vec<SysAction>,
) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("When the lid closes")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);
        ComboBox::from_id_salt("power5-lid-action")
            .selected_text(RichText::new(config.lid_action.label()).size(Style::SMALL))
            .show_ui(ui, |ui| {
                for action in LidAction::ALL {
                    let selected = config.lid_action == action;
                    if ui
                        .selectable_label(selected, RichText::new(action.label()).size(Style::SMALL))
                        .clicked()
                        && !selected
                    {
                        config.lid_action = action;
                        actions.push(SysAction::SavePowerHonorConfig);
                    }
                }
            });
    });
    muted_note(
        ui,
        "A desktop with no lid device honestly ignores this (nothing to close).",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_egui::egui::{pos2, vec2, Rect};
    use mde_seat::{Backend, BatteryKind, BatteryState};

    fn profiles(active: &str, available: &[&str]) -> ProfileState {
        ProfileState {
            active: active.to_owned(),
            available: available.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    fn battery(tte: Option<u64>, ttf: Option<u64>, rate: Option<f64>) -> Battery {
        Battery {
            model: "BAT0".to_owned(),
            kind: BatteryKind::Internal,
            percentage: 61.0,
            state: BatteryState::Discharging,
            power_supply: true,
            time_to_empty: tte.map(Duration::from_secs),
            time_to_full: ttf.map(Duration::from_secs),
            energy_rate: rate,
        }
    }

    #[test]
    fn profile_labels_prettify_known_ids_and_pass_unknown_through() {
        assert_eq!(profile_label("power-saver"), "Power-saver");
        assert_eq!(profile_label("balanced"), "Balanced");
        assert_eq!(profile_label("performance"), "Performance");
        // An id the daemon invents is shown verbatim, never dropped or guessed.
        assert_eq!(profile_label("turbo-max"), "turbo-max");
    }

    #[test]
    fn profile_select_dispatches_set_power_profile_with_the_chosen_name() {
        let state = profiles("balanced", &["power-saver", "balanced", "performance"]);
        // Selecting an offered, non-active profile emits exactly that switch.
        assert!(
            matches!(
                profile_action(&state, "performance"),
                Some(SysAction::SetPowerProfile(name)) if name == "performance"
            ),
            "expected SetPowerProfile(performance)"
        );
        // The already-active profile never re-dispatches (§7: no inert re-send).
        assert!(profile_action(&state, "balanced").is_none());
        // A profile the daemon does not advertise is never sent.
        assert!(profile_action(&state, "turbo").is_none());
    }

    #[test]
    fn the_ac_source_line_reads_honestly() {
        assert_eq!(ac_source_label(Some(true)), "On AC power");
        assert_eq!(ac_source_label(Some(false)), "On battery");
        // No LinePower adapter tracked → honest unknown, never a fabricated on-AC.
        assert_eq!(ac_source_label(None), "Power source unknown");
    }

    #[test]
    fn durations_format_as_a_compact_eta() {
        assert_eq!(format_duration(Duration::from_secs(8040)), "~2h 14m");
        assert_eq!(format_duration(Duration::from_secs(5400)), "~1h 30m");
        assert_eq!(format_duration(Duration::from_secs(2700)), "~45m");
        // Sub-minute is honestly "<1m", never a bare "~0m".
        assert_eq!(format_duration(Duration::from_secs(30)), "~<1m");
    }

    #[test]
    fn energy_rate_formats_as_watts() {
        assert_eq!(format_energy_rate(11.7), "11.7 W");
        assert_eq!(format_energy_rate(4.0), "4.0 W");
    }

    #[test]
    fn battery_telemetry_joins_present_fields_and_omits_absent() {
        // Discharging: an ETA-to-empty + a live draw, no time-to-full.
        let d = battery_telemetry(&battery(Some(5400), None, Some(11.7))).expect("some telemetry");
        assert!(d.contains("~1h 30m to empty"), "{d}");
        assert!(d.contains("11.7 W"), "{d}");
        assert!(!d.contains("to full"), "{d}");

        // Charging: the mirror case — a time-to-full appears.
        let c = battery_telemetry(&battery(None, Some(1800), Some(9.0))).expect("some telemetry");
        assert!(c.contains("~30m to full"), "{c}");

        // Nothing reported → an honest omission (no line at all), never a
        // fabricated 0 W / 0s (§7).
        assert!(battery_telemetry(&battery(None, None, None)).is_none());
    }

    #[test]
    fn a_refused_charge_write_including_eacces_folds_to_an_honest_message() {
        // The EACCES on the root-owned charge_control_end_threshold — the real
        // "not permitted" a non-root DRM session hits — is surfaced verbatim.
        let eacces = SeatError::Io {
            backend: Backend::ChargeThreshold,
            path: std::path::PathBuf::from(
                "/sys/class/power_supply/BAT0/charge_control_end_threshold",
            ),
            source: std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Permission denied (os error 13)",
            ),
        };
        let msg = charge_error(&eacces);
        assert!(msg.contains("Charge limit"), "{msg}");
        assert!(msg.to_lowercase().contains("permission denied"), "{msg}");

        // A machine with no advertising battery folds honestly too.
        let unsupported = SeatError::Unavailable {
            backend: Backend::ChargeThreshold,
            reason: "no battery advertises charge_control_end_threshold".into(),
        };
        assert!(charge_error(&unsupported).contains("not available"));
    }

    #[test]
    fn an_absent_profile_daemon_folds_to_an_honest_message() {
        let e = SeatError::Unavailable {
            backend: Backend::PowerProfiles,
            reason: "org.freedesktop.DBus.Error.ServiceUnknown".into(),
        };
        let msg = profile_error(&e);
        assert!(msg.contains("Power profile"), "{msg}");
        assert!(msg.contains("not available"), "{msg}");
    }

    /// Drive one headless egui frame over the given body, tessellating on the CPU
    /// (the DRM runner's path minus the GPU), and report whether it drew geometry.
    fn paints(mut build: impl FnMut(&mut egui::Ui)) -> bool {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 480.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| build(ui));
        });
        !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty()
    }

    #[test]
    fn the_power4_body_builders_lay_out_real_geometry_and_dispatch_nothing_idle() {
        let mut actions: Vec<SysAction> = Vec::new();
        let mut live: Option<u8> = None;
        let drew = paints(|ui| {
            let state = profiles("balanced", &["power-saver", "balanced", "performance"]);
            profile_body(ui, &state, &mut actions);
            // The honest empty-available state renders a note, not a blank.
            profile_body(ui, &profiles("", &[]), &mut actions);
            ac_source_body(ui, Some(true));
            ac_source_body(ui, Some(false));
            ac_source_body(ui, None);
            // Supported: a live slider. Unsupported: an honest note.
            charge_threshold_body(ui, Some(80), &mut live, &mut actions);
            charge_threshold_body(ui, None, &mut live, &mut actions);
        });
        assert!(drew, "the POWER-4 controls drew nothing");
        // No pointer input was injected, so no control may dispatch a write (§7:
        // controls act on real interaction, never spuriously on paint).
        assert!(
            actions.is_empty(),
            "an untouched frame must not dispatch an action"
        );
        // The supported slider seeded its live value from the probe (80).
        assert_eq!(live, Some(80));
    }

    #[test]
    fn the_idle_option_labels_read_never_and_minutes() {
        assert_eq!(idle_option_label(None), "Never");
        assert_eq!(idle_option_label(Some(1)), "1 min");
        assert_eq!(idle_option_label(Some(30)), "30 min");
    }

    #[test]
    fn the_power5_pickers_draw_and_dispatch_nothing_on_an_untouched_frame() {
        let mut actions: Vec<SysAction> = Vec::new();
        let mut config = PowerHonorConfig::default();
        let drew = paints(|ui| {
            idle_timeout_body(ui, &mut config, &mut actions);
            lid_action_body(ui, &mut config, &mut actions);
        });
        assert!(drew, "the POWER-5 pickers drew nothing");
        // No interaction was injected → no config write may dispatch (§7: a picker
        // acts on a real selection, never spuriously on paint).
        assert!(
            actions.is_empty(),
            "an untouched frame must not dispatch a save"
        );
        // The safe defaults are unchanged by a mere paint.
        assert_eq!(config, PowerHonorConfig::default());
    }
}
