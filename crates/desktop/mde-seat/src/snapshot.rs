//! The seat aggregator — one [`Seat`] holding every client, and [`Seat::snapshot`]
//! folding them into a [`SeatSnapshot`] of typed [`Probe`] states.
//!
//! This is the single model the shell's System surface and chrome status icons
//! render from (E12-15). Every section is a [`Probe`]: either `Present` with the
//! real reading, or `Absent` carrying the typed backend + reason — so an absent
//! backend renders as an honest "not available" line (§7 / interlock 4), never a
//! blank or a fake control. `snapshot()` never fails as a whole: a per-client
//! error becomes that section's `Absent`, so one dead backend never blanks the
//! rest of the panel.

use crate::backlight::{Backlight, BacklightClient, SysfsBacklight};
use crate::bluez::{BluezClient, BtStatus, ZbusBluez};
use crate::charge_threshold::{ChargeThresholdClient, SysfsChargeThreshold};
use crate::ddc::{DdcClient, DdcCtl, DdcDisplay};
use crate::display::{Connector, DisplayProber, DrmProber};
use crate::error::{Backend, SeatError};
use crate::lid::{LidClient, LidState, ProcLid};
use crate::logind::{LogindClient, PowerCaps, PowerVerb, ZbusLogind};
use crate::mixer::{MixerClient, MixerStatus, PwGraph};
use crate::powerprofiles::{ProfileState, ProfilesClient, ZbusProfiles};
use crate::upower::{Battery, UPowerClient, ZbusUPower};

/// A typed per-section state: a real reading, or a typed absence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe<T> {
    /// The backend answered with a reading.
    Present(T),
    /// The backend is absent or failed — the honest not-available render.
    Absent {
        /// Which backend.
        backend: Backend,
        /// Why (the typed error's message).
        reason: String,
    },
}

impl<T> Probe<T> {
    /// Fold a client `Result` into a `Probe`: `Ok` → `Present`, any typed error →
    /// `Absent` carrying its backend + message.
    #[must_use]
    pub fn from_result(r: Result<T, SeatError>) -> Self {
        match r {
            Ok(v) => Self::Present(v),
            Err(e) => Self::Absent {
                backend: e.backend(),
                reason: e.to_string(),
            },
        }
    }

    /// The reading, when present.
    #[must_use]
    pub const fn present(&self) -> Option<&T> {
        match self {
            Self::Present(v) => Some(v),
            Self::Absent { .. } => None,
        }
    }

    /// Whether a reading is present.
    #[must_use]
    pub const fn is_present(&self) -> bool {
        matches!(self, Self::Present(_))
    }
}

/// Every seat section, each a typed [`Probe`]. The one render model (E12-15).
#[derive(Debug, Clone)]
pub struct SeatSnapshot {
    /// Bluetooth adapter/device status.
    pub bluetooth: Probe<BtStatus>,
    /// Power devices (multi-battery incl. peripherals).
    pub batteries: Probe<Vec<Battery>>,
    /// On external (AC) power, from the `UPower` `LinePower` adapter's `Online`
    /// reading: `Present(Some(true))` on AC, `Present(Some(false))` on battery,
    /// `Present(None)` when no adapter is tracked (a desktop — AC state unknown),
    /// `Absent` when `UPower`/the bus is down.
    pub on_ac: Probe<Option<bool>>,
    /// logind power capabilities (which verbs are available).
    pub power: Probe<PowerCaps>,
    /// The active + available power profiles (`net.hadess.PowerProfiles`).
    /// `Absent` when power-profiles-daemon is not running — the honest
    /// "unavailable", never a fabricated active profile (§7).
    pub power_profile: Probe<ProfileState>,
    /// The battery charge-stop cap (`charge_control_end_threshold`, 0–100):
    /// `Present(Some(pct))` when a battery advertises it, `Present(None)` when
    /// the power-supply class exists but no battery has the attribute (most
    /// machines), `Absent` when there is no power-supply class at all.
    pub charge_limit: Probe<Option<u8>>,
    /// The laptop lid state (`/proc/acpi/button/lid`). `Absent` on a desktop
    /// (no lid device) — never a fabricated "open".
    pub lid: Probe<LidState>,
    /// DRM connectors + their modes (read-only probe).
    pub displays: Probe<Vec<Connector>>,
    /// sysfs backlight panels.
    pub backlights: Probe<Vec<Backlight>>,
    /// The audio mixer (`PipeWire` graph via [`PwGraph`]; `Absent` when no
    /// `PipeWire`/`pw-dump` is present, e.g. a headless host).
    pub mixer: Probe<MixerStatus>,
    /// DDC/CI external monitors (`Absent` until E12-18).
    pub ddc: Probe<Vec<DdcDisplay>>,
}

/// Holds every seat client behind its trait seam and folds them into a snapshot.
/// [`Seat::new`] wires the production clients; tests inject fakes via
/// [`Seat::from_parts`].
pub struct Seat {
    bluez: Box<dyn BluezClient>,
    upower: Box<dyn UPowerClient>,
    logind: Box<dyn LogindClient>,
    display: Box<dyn DisplayProber>,
    backlight: Box<dyn BacklightClient>,
    mixer: Box<dyn MixerClient>,
    ddc: Box<dyn DdcClient>,
    profiles: Box<dyn ProfilesClient>,
    charge: Box<dyn ChargeThresholdClient>,
    lid: Box<dyn LidClient>,
}

impl Seat {
    /// A seat over the real host: system-bus BlueZ/UPower/logind, the DRM prober,
    /// sysfs backlight, the `PipeWire` mixer (E12-16), and the DDC/CI client over
    /// `ddcutil` (E12-18).
    #[must_use]
    pub fn new() -> Self {
        Self {
            bluez: Box::new(ZbusBluez::new()),
            upower: Box::new(ZbusUPower::new()),
            logind: Box::new(ZbusLogind::new()),
            display: Box::new(DrmProber::new()),
            backlight: Box::new(SysfsBacklight::new()),
            mixer: Box::new(PwGraph::new()),
            ddc: Box::new(DdcCtl::new()),
            profiles: Box::new(ZbusProfiles::new()),
            charge: Box::new(SysfsChargeThreshold::new()),
            lid: Box::new(ProcLid::new()),
        }
    }

    /// Assemble a seat from explicit clients (the test seam).
    #[must_use]
    #[allow(
        clippy::too_many_arguments,
        reason = "the test seam mirrors every injectable client one-to-one"
    )]
    pub fn from_parts(
        bluez: Box<dyn BluezClient>,
        upower: Box<dyn UPowerClient>,
        logind: Box<dyn LogindClient>,
        display: Box<dyn DisplayProber>,
        backlight: Box<dyn BacklightClient>,
        mixer: Box<dyn MixerClient>,
        ddc: Box<dyn DdcClient>,
        profiles: Box<dyn ProfilesClient>,
        charge: Box<dyn ChargeThresholdClient>,
        lid: Box<dyn LidClient>,
    ) -> Self {
        Self {
            bluez,
            upower,
            logind,
            display,
            backlight,
            mixer,
            ddc,
            profiles,
            charge,
            lid,
        }
    }

    /// Fold every client into a [`SeatSnapshot`]. Infallible as a whole: each
    /// section independently becomes `Present` or a typed `Absent`.
    #[must_use]
    pub fn snapshot(&self) -> SeatSnapshot {
        SeatSnapshot {
            bluetooth: Probe::from_result(self.bluez.status()),
            batteries: Probe::from_result(self.upower.batteries()),
            on_ac: Probe::from_result(self.upower.on_ac()),
            power: Probe::from_result(self.logind.caps()),
            power_profile: Probe::from_result(self.profiles.state()),
            charge_limit: Probe::from_result(self.charge.end_threshold()),
            lid: Probe::from_result(self.lid.state()),
            displays: Probe::from_result(self.display.connectors()),
            backlights: Probe::from_result(self.backlight.devices()),
            mixer: Probe::from_result(self.mixer.status()),
            ddc: Probe::from_result(self.ddc.displays()),
        }
    }

    // ── control verbs (E12-18: the Displays + Power sections act through these) ──
    //
    // The shell holds the ONE seat (lock 1) and drives hardware in-process through
    // these; the same methods are what E12-19's `host_state` worker calls for the
    // remote allowlisted verbs. Confirm-gating (power) + the last-console interlock
    // (displays) are the caller's duty — see `PowerVerb::needs_confirm` and
    // `DisplayLayout::guard_disable`.

    /// Set a sysfs backlight panel's raw brightness (lock 13, internal panels).
    ///
    /// # Errors
    /// The backlight client's typed errors ([`SeatError::OutOfRange`] above the
    /// device max, [`SeatError::Unavailable`] when absent).
    pub fn set_backlight(&self, name: &str, value: u32) -> Result<(), SeatError> {
        self.backlight.set_brightness(name, value)
    }

    /// Set an external monitor's DDC/CI brightness (0–100), keyed by i2c bus label
    /// (lock 13, external monitors).
    ///
    /// # Errors
    /// The DDC client's typed errors ([`SeatError::Backend`] when the monitor
    /// rejects DDC, [`SeatError::Unavailable`] when `ddcutil` is absent).
    pub fn set_ddc_brightness(&self, bus: &str, percent: u8) -> Result<(), SeatError> {
        self.ddc.set_brightness(bus, percent)
    }

    /// Execute a logind power verb (lock 12). The caller has already confirm-gated
    /// it ([`PowerVerb::needs_confirm`]).
    ///
    /// # Errors
    /// The logind client's typed errors (a polkit refusal / absent logind).
    pub fn power(&self, verb: PowerVerb) -> Result<(), SeatError> {
        self.logind.act(verb)
    }

    /// Switch the active power profile (`net.hadess.PowerProfiles`), by name
    /// (`power-saver` / `balanced` / `performance`). The caller (POWER-4) reads
    /// [`SeatSnapshot::power_profile`] for the offered set first.
    ///
    /// # Errors
    /// The power-profiles client's typed errors ([`SeatError::Unavailable`] when
    /// power-profiles-daemon is absent, [`SeatError::Backend`] when it refuses an
    /// unknown name).
    pub fn set_power_profile(&self, name: &str) -> Result<(), SeatError> {
        self.profiles.set_active(name)
    }

    /// Set the battery charge-stop cap (`charge_control_end_threshold`, 0–100).
    ///
    /// # Errors
    /// The charge-threshold client's typed errors ([`SeatError::OutOfRange`]
    /// above 100, [`SeatError::Unavailable`] when no battery advertises the
    /// attribute, [`SeatError::Io`] on an unprivileged write — surfaced
    /// honestly, never a pretend success).
    pub fn set_charge_threshold(&self, pct: u8) -> Result<(), SeatError> {
        self.charge.set_end_threshold(pct)
    }

    /// Set a mixer strip's volume (0–100). Used by the mixer faders (E12-16) and
    /// the volume hotkeys (E12-19), which drive the master strip through here.
    ///
    /// # Errors
    /// The mixer client's typed errors ([`SeatError::Unavailable`] with no
    /// `PipeWire`, [`SeatError::Backend`] on a control failure).
    pub fn set_strip_volume(&self, strip_id: &str, volume: u8) -> Result<(), SeatError> {
        self.mixer.set_volume(strip_id, volume)
    }

    /// Set a mixer strip's mute. The mute/mic-mute hotkeys (E12-19) toggle the
    /// master (and, once modelled, the capture) strip through here.
    ///
    /// # Errors
    /// The mixer client's typed errors (as [`Self::set_strip_volume`]).
    pub fn set_strip_muted(&self, strip_id: &str, muted: bool) -> Result<(), SeatError> {
        self.mixer.set_muted(strip_id, muted)
    }

    /// Power a Bluetooth adapter's radio on/off (`Adapter1.Powered`). The BT-toggle
    /// hotkey (E12-19) and the remote BT verb (the `host_state` worker) both flip
    /// the adapter through here.
    ///
    /// # Errors
    /// The `BlueZ` client's typed errors (absent adapter / dead bus →
    /// [`SeatError::Unavailable`], else [`SeatError::Backend`]).
    pub fn set_bt_powered(&self, adapter: &str, on: bool) -> Result<(), SeatError> {
        self.bluez.set_adapter_powered(adapter, on)
    }

    // ── Bluetooth pairing-manager verbs (E12-17) ────────────────────────────────
    //
    // The System surface's Bluetooth panel drives the full manager through the ONE
    // seat's `BluezClient` (lock 1), the same seam the enumeration folds from. Each
    // forwards a typed `SeatError` (absent adapter/device or dead bus →
    // `Unavailable`, a refused write → `Backend`) so the panel surfaces an honest
    // failure, never a silent no-op. The PIN/passkey prompts a `pair` raises are
    // answered by a separately-registered [`crate::pairing::PairingAgent`].

    /// Make a Bluetooth adapter visible to nearby devices (`Adapter1.Discoverable`).
    ///
    /// # Errors
    /// The `BlueZ` client's typed errors (absent adapter / dead bus →
    /// [`SeatError::Unavailable`], else [`SeatError::Backend`]).
    pub fn set_bt_discoverable(&self, adapter: &str, on: bool) -> Result<(), SeatError> {
        self.bluez.set_discoverable(adapter, on)
    }

    /// Let a Bluetooth adapter accept incoming pairings (`Adapter1.Pairable`).
    ///
    /// # Errors
    /// As [`Self::set_bt_discoverable`].
    pub fn set_bt_pairable(&self, adapter: &str, on: bool) -> Result<(), SeatError> {
        self.bluez.set_pairable(adapter, on)
    }

    /// Start a device-discovery scan (`Adapter1.StartDiscovery`).
    ///
    /// # Errors
    /// As [`Self::set_bt_discoverable`].
    pub fn bt_start_discovery(&self, adapter: &str) -> Result<(), SeatError> {
        self.bluez.start_discovery(adapter)
    }

    /// Stop a device-discovery scan (`Adapter1.StopDiscovery`).
    ///
    /// # Errors
    /// As [`Self::set_bt_discoverable`].
    pub fn bt_stop_discovery(&self, adapter: &str) -> Result<(), SeatError> {
        self.bluez.stop_discovery(adapter)
    }

    /// Pair (bond) with a device (`Device1.Pair`). PIN/passkey prompts are answered
    /// by the registered [`crate::pairing::PairingAgent`].
    ///
    /// # Errors
    /// The `BlueZ` client's typed errors (absent device / bus → `Unavailable`, a
    /// rejected/failed pairing → `Backend`).
    pub fn bt_pair(&self, device: &str) -> Result<(), SeatError> {
        self.bluez.pair(device)
    }

    /// Abort an in-flight pairing (`Device1.CancelPairing`).
    ///
    /// # Errors
    /// As [`Self::bt_pair`].
    pub fn bt_cancel_pairing(&self, device: &str) -> Result<(), SeatError> {
        self.bluez.cancel_pairing(device)
    }

    /// Trust or untrust a device for auto-reconnect (`Device1.Trusted`).
    ///
    /// # Errors
    /// The `BlueZ` client's typed errors (absent device / bus → `Unavailable`, else
    /// `Backend`).
    pub fn set_bt_trusted(&self, device: &str, trusted: bool) -> Result<(), SeatError> {
        self.bluez.set_trusted(device, trusted)
    }

    /// Connect to a paired device (`Device1.Connect`).
    ///
    /// # Errors
    /// As [`Self::set_bt_trusted`].
    pub fn bt_connect(&self, device: &str) -> Result<(), SeatError> {
        self.bluez.connect(device)
    }

    /// Disconnect a connected device (`Device1.Disconnect`).
    ///
    /// # Errors
    /// As [`Self::set_bt_trusted`].
    pub fn bt_disconnect(&self, device: &str) -> Result<(), SeatError> {
        self.bluez.disconnect(device)
    }

    /// Forget a device — drop the bond + link keys (`Adapter1.RemoveDevice`).
    /// `adapter` is the owning adapter path; `device` the device path to remove.
    ///
    /// # Errors
    /// An invalid `device` path → [`SeatError::Protocol`]; absent adapter / bus →
    /// `Unavailable`, else `Backend`.
    pub fn bt_remove_device(&self, adapter: &str, device: &str) -> Result<(), SeatError> {
        self.bluez.remove_device(adapter, device)
    }
}

impl Default for Seat {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_result_folds_ok_to_present_and_err_to_absent_with_backend() {
        let ok: Probe<u8> = Probe::from_result(Ok(7));
        assert!(ok.is_present());
        assert_eq!(ok.present(), Some(&7));

        let err: Probe<u8> = Probe::from_result(Err(SeatError::Unavailable {
            backend: Backend::PipeWire,
            reason: "no daemon".into(),
        }));
        assert!(!err.is_present());
        match err {
            Probe::Absent { backend, reason } => {
                assert_eq!(backend, Backend::PipeWire);
                assert!(reason.contains("PipeWire"));
            }
            Probe::Present(_) => panic!("expected Absent"),
        }
    }

    #[test]
    fn the_control_verbs_answer_typed_on_a_headless_seat_never_panic() {
        // On the headless farm host every backend is Absent, so each control verb
        // (the ones the hotkeys + the remote host_state worker drive) must return a
        // typed SeatError tagged its own backend — never panic, never a fake Ok.
        let seat = Seat::new();
        match seat.set_strip_volume("0", 50) {
            Ok(()) => {}
            Err(e) => assert_eq!(e.backend(), Backend::PipeWire),
        }
        match seat.set_strip_muted("0", true) {
            Ok(()) => {}
            Err(e) => assert_eq!(e.backend(), Backend::PipeWire),
        }
        // The BT pairing-manager verbs (E12-17) all forward through the one seat;
        // each folds to a typed Bluetooth error on a host with no adapter/bus.
        let bt = |r: Result<(), SeatError>| {
            if let Err(e) = r {
                assert_eq!(e.backend(), Backend::Bluetooth);
            }
        };
        bt(seat.set_bt_powered("/org/bluez/hci0", true));
        bt(seat.set_bt_discoverable("/org/bluez/hci0", true));
        bt(seat.set_bt_pairable("/org/bluez/hci0", true));
        bt(seat.bt_start_discovery("/org/bluez/hci0"));
        bt(seat.bt_stop_discovery("/org/bluez/hci0"));
        bt(seat.bt_pair("/org/bluez/hci0/dev_AA_BB"));
        bt(seat.bt_cancel_pairing("/org/bluez/hci0/dev_AA_BB"));
        bt(seat.set_bt_trusted("/org/bluez/hci0/dev_AA_BB", true));
        bt(seat.bt_connect("/org/bluez/hci0/dev_AA_BB"));
        bt(seat.bt_disconnect("/org/bluez/hci0/dev_AA_BB"));
        bt(seat.bt_remove_device("/org/bluez/hci0", "/org/bluez/hci0/dev_AA_BB"));
    }

    #[test]
    fn a_real_seat_snapshots_without_panicking_and_every_bound_section_answers_typed() {
        // On the headless build host every D-Bus/DRM section is legitimately
        // Absent; the point is snapshot() never panics. The mixer drives the real
        // PipeWire client (E12-16) and DDC the real ddcutil client (E12-18): with
        // no pw-dump / no ddcutil each is Absent tagged its own backend, with a live
        // backend Present — either way typed, never a fabricated reading.
        let snap = Seat::new().snapshot();
        if let Probe::Absent { backend, .. } = &snap.mixer {
            assert_eq!(*backend, Backend::PipeWire);
        }
        if let Probe::Absent { backend, .. } = &snap.ddc {
            assert_eq!(*backend, Backend::Ddc);
        }
        // The on-AC read shares UPower's backend: a headless host has no bus, so
        // it is Absent tagged UPower; a live host answers Present(Some/None).
        if let Probe::Absent { backend, .. } = &snap.on_ac {
            assert_eq!(*backend, Backend::UPower);
        }
        // The POWER-3 backends: each is legitimately Absent on the headless host
        // (no power-profiles-daemon, no power-supply class, no lid button), and
        // every Absent carries its own backend — never a fabricated reading (§7).
        if let Probe::Absent { backend, .. } = &snap.power_profile {
            assert_eq!(*backend, Backend::PowerProfiles);
        }
        if let Probe::Absent { backend, .. } = &snap.charge_limit {
            assert_eq!(*backend, Backend::ChargeThreshold);
        }
        if let Probe::Absent { backend, .. } = &snap.lid {
            assert_eq!(*backend, Backend::Lid);
        }
    }

    #[test]
    fn the_power3_drive_methods_answer_typed_on_a_headless_seat_never_panic() {
        // The profile switch + charge-cap write (POWER-4 drives these) fold to a
        // typed error tagged their own backend on a host without the daemon /
        // the sysfs attribute — never a panic, never a fake Ok.
        let seat = Seat::new();
        match seat.set_power_profile("balanced") {
            Ok(()) => {}
            Err(e) => assert_eq!(e.backend(), Backend::PowerProfiles),
        }
        match seat.set_charge_threshold(80) {
            Ok(()) => {}
            Err(e) => assert_eq!(e.backend(), Backend::ChargeThreshold),
        }
        // An over-100 cap is refused OutOfRange before any I/O, regardless of host.
        assert!(matches!(
            seat.set_charge_threshold(150),
            Err(SeatError::OutOfRange {
                backend: Backend::ChargeThreshold,
                max: 100,
                ..
            })
        ));
    }
}
