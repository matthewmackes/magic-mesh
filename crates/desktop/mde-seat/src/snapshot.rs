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
use crate::ddc::{DdcClient, DdcDisplay, UnboundDdc};
use crate::display::{Connector, DisplayProber, DrmProber};
use crate::error::{Backend, SeatError};
use crate::logind::{LogindClient, PowerCaps, ZbusLogind};
use crate::mixer::{MixerClient, MixerStatus, PwGraph};
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
    /// logind power capabilities (which verbs are available).
    pub power: Probe<PowerCaps>,
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
}

impl Seat {
    /// A seat over the real host: system-bus BlueZ/UPower/logind, the DRM prober,
    /// sysfs backlight, the `PipeWire` mixer (E12-16), and the not-yet-bound DDC
    /// seam (E12-18).
    #[must_use]
    pub fn new() -> Self {
        Self {
            bluez: Box::new(ZbusBluez::new()),
            upower: Box::new(ZbusUPower::new()),
            logind: Box::new(ZbusLogind::new()),
            display: Box::new(DrmProber::new()),
            backlight: Box::new(SysfsBacklight::new()),
            mixer: Box::new(PwGraph::new()),
            ddc: Box::new(UnboundDdc),
        }
    }

    /// Assemble a seat from explicit clients (the test seam).
    #[must_use]
    pub fn from_parts(
        bluez: Box<dyn BluezClient>,
        upower: Box<dyn UPowerClient>,
        logind: Box<dyn LogindClient>,
        display: Box<dyn DisplayProber>,
        backlight: Box<dyn BacklightClient>,
        mixer: Box<dyn MixerClient>,
        ddc: Box<dyn DdcClient>,
    ) -> Self {
        Self {
            bluez,
            upower,
            logind,
            display,
            backlight,
            mixer,
            ddc,
        }
    }

    /// Fold every client into a [`SeatSnapshot`]. Infallible as a whole: each
    /// section independently becomes `Present` or a typed `Absent`.
    #[must_use]
    pub fn snapshot(&self) -> SeatSnapshot {
        SeatSnapshot {
            bluetooth: Probe::from_result(self.bluez.status()),
            batteries: Probe::from_result(self.upower.batteries()),
            power: Probe::from_result(self.logind.caps()),
            displays: Probe::from_result(self.display.connectors()),
            backlights: Probe::from_result(self.backlight.devices()),
            mixer: Probe::from_result(self.mixer.status()),
            ddc: Probe::from_result(self.ddc.displays()),
        }
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
    fn a_real_seat_snapshots_without_panicking_and_the_mixer_answers_typed() {
        // On the headless build host every D-Bus/DRM section is legitimately
        // Absent; the point is snapshot() never panics. The mixer now drives the
        // real PipeWire client (E12-16): with no pw-dump it is Absent(PipeWire),
        // with a live graph it is Present — either way tagged PipeWire, never a
        // fabricated reading. The DDC seam stays Absent until E12-18.
        let snap = Seat::new().snapshot();
        if let Probe::Absent { backend, .. } = &snap.mixer {
            assert_eq!(*backend, Backend::PipeWire);
        }
        assert!(!snap.ddc.is_present(), "ddc must be Absent until E12-18");
    }
}
